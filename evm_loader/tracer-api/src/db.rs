use clickhouse::Client;
use thiserror::Error;

use evm::{H160, H256};
use solana_account_decoder::parse_token::{
    parse_token, TokenAccountType, UiTokenAccount, UiTokenAmount,
};
use solana_client::rpc_response::{Response, RpcResponseContext};
use solana_sdk::account::{Account, ReadableAccount};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use tokio::task::block_in_place;
use tracing::debug;

use crate::types::TxMeta;
use crate::utils::parse_token_amount;

type Slot = u64;

pub struct DbClient {
    client: Client,
}

impl std::fmt::Debug for DbClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbClient{{}}")
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("clickhouse: {}", .0)]
    Db(#[from] clickhouse::error::Error),
}

macro_rules! field_with_meta {
    ($field:literal) => {
        concat!(
            "
            (slot, eth_from_addr, eth_to_addr,
            toFixedString(unhex(reinterpretAsString(eth_transaction_signature)), 32)
             as eth_transaction_signature,",
            $field,
            " as value)"
        )
    };
}

#[derive(clickhouse::Row, serde::Deserialize)]
struct TxMetaRow<T> {
    slot: u64,
    eth_from_addr: [u8; 20],
    eth_to_addr: Option<[u8; 20]>,
    eth_transaction_signature: [u8; 32],
    value: T,
}

impl<T> TxMetaRow<T> {
    fn into_meta_with<U>(self, f: impl FnOnce(T) -> U) -> TxMeta<U> {
        TxMeta {
            slot: self.slot,
            from: self.eth_from_addr.into(),
            to: self.eth_to_addr.map(Into::into),
            eth_signature: self.eth_transaction_signature.into(),
            value: f(self.value),
        }
    }
}

impl<T, U: From<T>> From<TxMetaRow<T>> for TxMeta<U> {
    fn from(item: TxMetaRow<T>) -> Self {
        item.into_meta_with(From::from)
    }
}

#[derive(Debug, serde::Deserialize, clickhouse::Row, Clone)]
struct AccountRow {
    pubkey: [u8; 32],
    lamports: u64,
    data: Vec<u8>,
    owner: [u8; 32],
    executable: bool,
    rent_epoch: u64,
}

impl From<AccountRow> for Account {
    fn from(row: AccountRow) -> Account {
        Account {
            lamports: row.lamports,
            data: row.data,
            owner: Pubkey::new_from_array(row.owner),
            executable: row.executable,
            rent_epoch: row.rent_epoch,
        }
    }
}

type DbResult<T> = std::result::Result<T, Error>;

impl DbClient {
    pub fn new(
        addr: impl Into<String>,
        user: Option<String>,
        password: Option<String>,
        db: Option<String>,
    ) -> Self {
        let client = Client::default().with_url(addr);
        let client = if let Some(user) = user {
            client.with_user(user)
        } else {
            client
        };
        let client = if let Some(password) = password {
            client.with_password(password)
        } else {
            client
        };
        let client = if let Some(db) = db {
            client.with_database(db)
        } else {
            client
        };
        DbClient { client }
    }

    #[tracing::instrument]
    pub fn get_transactions_by_slot(&self, slot: Slot) -> Result<Vec<TxMeta<Message>>, Error> {
        let msgs = self.block(|client| async move {
            client
                .query(&format!(
                    "SELECT {}
                          FROM transactions T
                          LEFT ANY JOIN evm_transactions E
                          ON E.transaction_signature = T.transaction_signature
                          WHERE slot = ?",
                    field_with_meta!("message")
                ))
                .bind(slot)
                .fetch_all::<TxMetaRow<Vec<u8>>>()
                .await
        })?;
        let msgs = msgs
            .into_iter()
            .map(|row| row.into_meta_with(|data| bincode::deserialize(&data).unwrap()))
            .collect();
        Ok(msgs)
    }

    #[tracing::instrument]
    pub fn get_transactions(
        &self,
        from: Option<u64>,
        to: Option<u64>,
        from_addr: Option<Vec<H160>>,
        to_addr: Option<Vec<H160>>,
        offset: Option<usize>,
        count: Option<usize>,
    ) -> Result<Vec<TxMeta<Message>>, Error> {
        let b2i = |b| if b { 1 } else { 0 };
        let addrs_to_str = |addrs: Vec<H160>| {
            addrs
                .iter()
                .map(|addr| hex::encode(addr.as_bytes()))
                .fold(String::new(), |old, addr| {
                    format!("{} unhex('{}')", old, addr)
                })
        };
        let from_addrs = from_addr
            .map(addrs_to_str)
            .map(|s| format!("AND E.eth_from_addr IN ({})", s))
            .unwrap_or_default();
        let to_addrs = to_addr
            .map(addrs_to_str)
            .map(|s| format!("AND E.eth_to_addr IN ({})", s))
            .unwrap_or_default();

        let msgs = self.block(|client| async move {
            let query = format!(
                "
                SELECT {}
                FROM evm_transactions E
                JOIN transactions T
                ON T.transaction_signature = E.transaction_signature
                WHERE
                ((slot >= ?) OR ?)
                AND ((slot <= ?) OR ?)
                {} {}
                OFFSET ?",
                field_with_meta!("message"),
                from_addrs,
                to_addrs
            );

            client
                .query(&query)
                .bind(from.unwrap_or(0))
                .bind(b2i(from.is_none()))
                .bind(to.unwrap_or(0))
                .bind(b2i(to.is_none()))
                .bind(offset.unwrap_or(0) as u64)
                .fetch_all::<TxMetaRow<Vec<u8>>>()
                .await
        })?;
        let msgs = msgs
            .into_iter()
            .map(|row| row.into_meta_with(|data| bincode::deserialize(&data).unwrap())) // TODO
            .collect();
        Ok(msgs)
    }

    #[tracing::instrument]
    pub fn get_transaction_data(&self, tx: H256) -> Result<Option<TxMeta<Message>>, Error> {
        let tx = hex::encode(tx.as_bytes());
        let msgs = self.block(|client| async move {
            client
                .query(&format!(
                    "SELECT {}
                        FROM transactions T, evm_transactions E
                        WHERE transaction_signature IN
                        (
                         SELECT transaction_signature
                         FROM evm_transactions
                         WHERE eth_transaction_signature = ?
                        )",
                    field_with_meta!("message")
                ))
                .bind(tx)
                .fetch_all::<TxMetaRow<Vec<u8>>>()
                .await
        })?;
        let msg = msgs
            .into_iter()
            .nth(0)
            .map(|row| row.into_meta_with(|msg| bincode::deserialize(&msg).unwrap())); // TODO
        Ok(msg)
    }

    #[tracing::instrument]
    pub fn get_slot_for_tx(&self, tx: H256) -> Result<Option<u64>, Error> {
        let slot = self.block(|client| async move {
            client
                .query(
                    "SELECT min(slot)
                     FROM transactions
                     WHERE transaction_signature IN
                     (
                       SELECT transaction_signature
                       FROM evm_transactions
                       WHERE eth_transaction_signature = ?
                     )",
                )
                .bind(hex::encode(tx.as_bytes()))
                .fetch_one::<u64>()
                .await
        })?;
        Ok(Some(slot))
    }

    fn block<F, Fu, R>(&self, f: F) -> R
    where
        F: FnOnce(Client) -> Fu,
        Fu: std::future::Future<Output = R>,
    {
        let client = self.client.clone();
        block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(f(client))
        })
    }

    #[tracing::instrument]
    pub fn get_slot(&self) -> Result<Slot, Error> {
        let slot = self.block(|client| async move {
            client
                .query("SELECT max(slot) FROM transactions")
                .fetch_one::<u64>()
                .await
        })?;
        Ok(slot)
    }

    #[tracing::instrument]
    pub fn get_block_time(&self, slot: Slot) -> Result<i64, Error> {
        let time = self.block(|client| async move {
            client
                .query("SELECT toUnixTimestamp(date_time) from transactions where slot = ?")
                .bind(slot)
                .fetch_one::<i64>()
                .await
        })?;
        Ok(time)
    }

    pub fn get_accounts_for_tx(&self, tx: H256) -> DbResult<Vec<(Pubkey, Account)>> {
        let accounts = self.block(|client| async move {
            client
                .query(
                    "SELECT
                        public_key,
                        lamports,
                        data,
                        owner,
                        executable,
                        rent_epoch
                     FROM accounts
                     WHERE transaction_signature IN
                     (
                         SELECT transaction_signature
                         FROM evm_transactions
                         WHERE eth_transaction_signature = ?
                     )",
                )
                .bind(hex::encode(tx.as_bytes()))
                .fetch_all::<AccountRow>()
                .await
        })?;
        let accounts = accounts
            .into_iter()
            .map(|row| (Pubkey::new_from_array(row.pubkey), Account::from(row)))
            .collect();
        tracing::info!("found accounts: {:?}", accounts);
        Ok(accounts)
    }

    pub fn get_accounts_at_slot(
        &self,
        pubkeys: impl Iterator<Item = Pubkey>,
        slot: Slot,
    ) -> DbResult<Vec<(Pubkey, Account)>> {
        let pubkeys = pubkeys
            .map(|pubkey| hex::encode(&pubkey.to_bytes()[..]))
            .fold(String::new(), |old, addr| {
                format!("{} unhex('{}'),", old, addr)
            });

        let accounts = self.block(|client| async move {
            client
                .query(&format!(
                    "SELECT
                        public_key,
                        argMax(lamports, T.slot),
                        argMax(data, T.slot),
                        argMax(owner,T.slot),
                        argMax(executable,T.slot),
                        argMax(rent_epoch,T.slot)
                     FROM accounts A
                     JOIN transactions T
                     ON A.transaction_signature = T.transaction_signature
                     WHERE T.slot <= ? AND public_key IN ({})
                     GROUP BY public_key",
                    pubkeys
                ))
                .bind(slot)
                .fetch_all::<AccountRow>()
                .await
        })?;
        let accounts = accounts
            .into_iter()
            .map(|row| (Pubkey::new_from_array(row.pubkey), Account::from(row)))
            .collect();
        debug!("found account: {:?}", accounts);
        Ok(accounts)
    }

    #[tracing::instrument]
    pub fn get_account_at_slot(&self, pubkey: &Pubkey, slot: Slot) -> DbResult<Option<Account>> {
        let accounts = self.get_accounts_at_slot(std::iter::once(pubkey.to_owned()), slot)?;
        let account = accounts.get(0).map(|(_, account)| account).cloned();
        Ok(account)
    }

    #[tracing::instrument]
    pub fn get_token_account_balance_at_slot(
        &self,
        pubkey: &Pubkey,
        slot: Slot,
    ) -> DbResult<UiTokenAmount> {
        let account = self.get_account_at_slot(pubkey, slot)?.unwrap();
        let balance = parse_token_amount(&account).expect("could not parse token account");

        Ok(balance)
    }

    pub fn get_token_account_at_slot(
        &self,
        pubkey: &Pubkey,
        slot: Slot,
    ) -> DbResult<Option<UiTokenAccount>> {
        let account = self.get_account_at_slot(pubkey, slot)?.unwrap();

        let token = parse_token(account.data(), None).unwrap();
        match token {
            TokenAccountType::Account(acc) => Ok(Some(acc)),
            _ => todo!(),
        }
    }
}
