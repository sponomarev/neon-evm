use std::collections::HashMap;
use std::{borrow::BorrowMut, cell::RefCell, rc::Rc};

use tracing::warn;

use evm::backend::Apply;
use evm::{H160, U256};
use evm_loader::{
    account::{EthereumAccount, EthereumContract, ACCOUNT_SEED_VERSION},
    executor_state::{ERC20Approve, SplApprove, SplTransfer},
    solana_backend::{AccountStorage, AccountStorageInfo},
    solidity_account::SolidityAccount,
};

use evm_loader::account::tag;

use solana_program::instruction::AccountMeta;
use solana_sdk::{account::Account, pubkey::Pubkey};

use super::provider::Provider;
use crate::neon::{Config, EvmAccount};
use crate::utils::parse_token_amount;
use solana_sdk::account_info::AccountInfo;
use std::collections::{BTreeMap, BTreeSet};

pub fn make_solana_program_address(ether_address: &H160, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[&[ACCOUNT_SEED_VERSION], ether_address.as_bytes()],
        program_id,
    )
}

struct SolanaAccount {
    account: Account,
    code_account: Option<Account>,
    key: Pubkey,
    writable: bool,
    code_size: Option<usize>,
    balance: u64,
}

impl SolanaAccount {
    pub fn new(account: Account, key: Pubkey, balance: u64, code_account: Option<Account>) -> Self {
        eprintln!("SolanaAccount::new");
        Self {
            account,
            key,
            balance,
            writable: false,
            code_account,
            code_size: None,
        }
    }
}

macro_rules! bail_with_default {
    ($opt:expr, $fun:expr) => {
        match $opt {
            Some(value) => value,
            None => return $fun(),
        }
    };
}

#[allow(clippy::module_name_repetitions)]
pub struct EmulatorAccountStorage<'a, P> {
    ethereum_accounts: BTreeMap<H160, EvmAccount<'a>>,
    // accounts: RefCell<HashMap<H160, SolanaAccount>>,
    provider: P,
    contract_id: H160,
    caller_id: H160,
    block_number: u64,
    block_timestamp: i64,
}

impl<'a, P: Provider> EmulatorAccountStorage<'a, P> {
    pub fn new(
        provider: P,
        contract_id: H160,
        caller_id: H160,
        block_number: Option<u64>,
    ) -> EmulatorAccountStorage<P> {
        eprintln!("backend::new");

        let slot = block_number.unwrap_or_else(|| {
            if let Ok(slot) = provider.get_slot() {
                eprintln!("Got slot");
                eprintln!("Slot {}", slot);
                slot
            } else {
                eprintln!("Get slot error");
                0
            }
        });

        let timestamp = if let Ok(timestamp) = provider.get_block_time(slot) {
            eprintln!("Got timestamp");
            eprintln!("timestamp {}", timestamp);
            timestamp
        } else {
            eprintln!("Get timestamp error");
            0
        };

        Self {
            // accounts: RefCell::new(HashMap::new()),
            ethereum_accounts:  BTreeMap::new(),
            provider,
            contract_id,
            caller_id,
            block_number: slot,
            block_timestamp: timestamp,
        }
    }

    pub fn fetch_account(&self, pubkey: &Pubkey, slot: u64) -> Option<Account> {
        self.provider.get_account_at_slot(pubkey, slot).ok()? // TODO: warning
    }

    fn init_neon_account(&mut self, address: H160) {

        macro_rules! return_none {
            ($opt:expr) => {
                bail_with_default!($opt, || ())
            };
        }

        if !self.ethereum_accounts.contains_key(&address) {
            let program_id = self.provider.evm_loader();

            let (sol, _) =
                make_solana_program_address(&address, self.provider.evm_loader());

            // Note: CLI logic will add the address to new_accounts map.
            // Note: In our case we always work with created accounts.
            // TODO:  remove macro?
            let acc = return_none!(self.fetch_account(&sol, self.block_number));

            if acc.owner != program_id{
                return
            }

            let info = AccountInfo::from(&acc);

            match tag(program_id, &info){
                Ok(EthereumAccount::TAG) => {}
                Ok(_) | Err(_) => return
            }

            let ether_account = EthereumAccount::from_account(program_id, &info)?;

            let evm_account = if let Some(code_key) = ether_account.code_account {
                let acc = self.fetch_account(&code_key, self.block_number).
                    unwrap_or_else(
                        warn!(
                            neon_account_key = %solana_address,
                            code_account_key = %account_data.code_account,
                            "code account not found"
                        )
                    );
                let ether_contract = EthereumConrtact::from_account(program_id, &acc);
                EvmAccount::Contract(ether_account, ether_contract)

            }
            else{
                EvmAccount::User(ether_account)
            };

            self.ethereum_accounts.insert(ether_account, evm_account);
        }
    }


    pub fn ethereum_account(&mut self, address: &H160) -> Option<&EthereumAccount<'a>> {
        // TODO: check existance?
        self.init_neon_account(*address);

        match self.ethereum_accounts.get(address)? {
            Account::User(ref account) => Some(account),
            Account::Contract(ref account, _) => Some(account),
        }
    }

    pub fn ethereum_contract(&mut self, address: &H160) -> Option<&EthereumContract<'a>> {
        self.init_neon_account(*address);

        match self.ethereum_accounts.get(address)? {
            Account::User(_) => None,
            Account::Contract(_, ref contract) => Some(contract),
        }
    }

}

impl<'a, P: Provider> AccountStorage for EmulatorAccountStorage<'a, P> {
    // fn apply_to_account<U, D, F>(&self, address: &H160, d: D, f: F) -> U
    // where
    //     F: FnOnce(&SolidityAccount<'_>) -> U,
    //     D: FnOnce() -> U,
    // {
    //     macro_rules! ward {
    //         ($opt:expr) => {
    //             bail_with_default!($opt, d)
    //         };
    //     }
    //     self.init_neon_account(*address);
    //     let mut accounts = self.accounts.borrow_mut();
    //
    //     let account = ward!(accounts.get(address));
    //     let a_data = ward!(AccountData::unpack(&account.account.data)
    //         .ok()
    //         .filter(|data| matches!(data, AccountData::Account(_))));
    //
    //     let mut code_data;
    //     let mut code = None;
    //     if let Some(ref code_account) = account.code_account {
    //         code_data = code_account.data.clone();
    //         let contract_data = ward!(AccountData::unpack(&code_account.data)
    //             .ok()
    //             .filter(|data| matches!(data, AccountData::Contract(_))));
    //         let code_data = Rc::new(RefCell::new(code_data.as_mut()));
    //         code = Some((contract_data, code_data));
    //     }
    //
    //     let account = SolidityAccount::new(&account.key, a_data, code);
    //     f(&account)
    // }

    // fn apply_to_solana_account<U, D, F>(&self, address: &Pubkey, d: D, f: F) -> U
    // where
    //     F: FnOnce(&AccountStorageInfo) -> U,
    //     D: FnOnce() -> U,
    // {
    //     let mut account = bail_with_default!(self.fetch_account(address, self.block_number), d);
    //     f(&account_storage_info(&mut account))
    // }

    fn balance(&mut self, address: &H160) -> U256 {
        self.ethereum_account(address)
            .map_or_else(U256::zero, |a| a.balance)
    }

    fn program_id(&self) -> &Pubkey {
        &self.provider.evm_loader()
    }

    fn contract(&self) -> H160 {
        self.contract_id
    }

    fn origin(&self) -> H160 {
        self.caller_id
    }

    fn block_number(&self) -> U256 {
        self.block_number.into()
    }

    fn block_timestamp(&self) -> U256 {
        self.block_timestamp.into()
    }

    fn get_account_solana_address(&self, address: &H160) -> Pubkey {
        make_solana_program_address(address, &self.provider.evm_loader()).0
    }

    fn nonce(&mut self, address: &H160) -> U256 {
        self.ethereum_account(address)
            .map_or_else(U256::zero, |a| a.trx_count).into()
    }

    fn code(&mut self, address: &H160) -> Vec<u8> {
        self.ethereum_contract(address)
            .map(|c| &c.extension.code)
            .map_or_else(Vec::new, |code| code.to_vec())
    }

    fn storage(&mut self, address: &H160, index: &U256) -> U256 {
        self.ethereum_contract(address)
            .map(|c| &c.extension.storage)
            .and_then(|hamt| hamt.find(*index))
            .unwrap_or_else(U256::zero)
    }

}

// fn account_storage_info(account: &mut Account) -> AccountStorageInfo {
//     AccountStorageInfo {
//         lamports: account.lamports,
//         data: Rc::new(RefCell::new(&mut account.data)),
//         owner: &account.owner,
//         executable: account.executable,
//         rent_epoch: account.rent_epoch,
//     }
// }
