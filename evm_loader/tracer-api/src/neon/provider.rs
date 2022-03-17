use std::{borrow::Borrow, collections::HashMap, convert::Infallible, sync::Arc};

use solana_program::{clock::Slot, pubkey::Pubkey};
use solana_sdk::account::Account;

use crate::db::{DbClient, Error as DbError};

pub trait Provider {
    type Error: std::fmt::Display + std::error::Error + Send + Sync + 'static;

    fn get_account_at_slot(
        &self,
        pubkey: &Pubkey,
        slot: u64,
    ) -> Result<Option<Account>, Self::Error>;

    fn get_slot(&self) -> Result<Slot, Self::Error>;
    fn get_block_time(&self, slot: u64) -> Result<i64, Self::Error>; // TODO: Clock sysvar
    fn evm_loader(&self) -> &Pubkey;
}

pub struct DbProvider {
    client: Arc<DbClient>,
    evm_loader: Pubkey,
}

impl DbProvider {
    pub fn new(client: Arc<DbClient>, evm_loader: Pubkey) -> Self {
        Self { client, evm_loader }
    }
}

impl Provider for DbProvider {
    type Error = DbError;

    fn get_account_at_slot(
        &self,
        pubkey: &Pubkey,
        slot: u64,
    ) -> Result<Option<Account>, Self::Error> {
        self.client.get_account_at_slot(pubkey, slot)
    }

    fn get_slot(&self) -> Result<Slot, Self::Error> {
        self.client.get_slot()
    }

    fn get_block_time(&self, slot: u64) -> Result<i64, Self::Error> {
        self.client.get_block_time(slot)
    }

    fn evm_loader(&self) -> &Pubkey {
        &self.evm_loader
    }
}

pub struct MapProvider<M> {
    map: M,
    slot: Slot,
    evm_loader: Pubkey,
}

impl<M> MapProvider<M> {
    pub fn new(map: M, evm_loader: Pubkey, slot: Slot) -> Self {
        Self {
            map,
            evm_loader,
            slot,
        }
    }
}

impl<M> Provider for MapProvider<M>
where
    M: Borrow<HashMap<Pubkey, Account>>,
{
    type Error = Infallible;

    fn get_account_at_slot(
        &self,
        pubkey: &Pubkey,
        _slot: u64,
    ) -> Result<Option<Account>, Self::Error> {
        Ok(self.map.borrow().get(pubkey).cloned())
    }

    fn get_slot(&self) -> Result<Slot, Self::Error> {
        Ok(self.slot)
    }

    fn get_block_time(&self, slot: u64) -> Result<i64, Self::Error> {
        Ok(0)
    }

    fn evm_loader(&self) -> &Pubkey {
        &self.evm_loader
    }
}
