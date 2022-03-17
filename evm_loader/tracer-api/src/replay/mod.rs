/// TODO:
/// [] Logging & log collection
/// [] Compute budget meters
/// [] Revise defaults and constants
/// [] Revise FeatureSet interactions
/// [] ? Blockhash ?
mod native_loader;
mod processor;
mod system_program;

use std::collections::HashMap;

use solana_ledger::builtins;
use solana_program::bpf_loader_upgradeable;
use solana_program::{message::Message, pubkey::Pubkey, sysvar};
use solana_sdk::{account::Account, process_instruction::ProcessInstructionWithContext};

use native_loader::NativeLoader;
pub use processor::{Error, MessageProcessor, ProcessedMessage};

/// Initalize account map with dummy builtin accounts
/// TODO: check default values for fields
pub fn init_accounts_map(map: &mut HashMap<Pubkey, Account>) {
    for (pubkey, _) in builtins() {
        map.insert(
            pubkey,
            Account {
                lamports: 10_000,
                data: vec![],
                owner: solana_sdk::native_loader::id(),
                executable: true, // ???
                rent_epoch: 10_000,
            },
        );
    }
}

pub fn syscallable_sysvars() -> impl Iterator<Item = Pubkey> + 'static {
    static SYSCALLABLE_SYSVARS: [Pubkey; 4] = [
        sysvar::clock::ID,
        sysvar::epoch_schedule::ID,
        sysvar::fees::ID,
        sysvar::rent::ID,
    ];

    // TODO ?: Consider iterating all available sysvars
    // TODO ?: and filtering the ones that return `Ok` on `Sysvar::get`

    // TODO 2: Rust 2021 will allow to return just an array + `.into_iter()`

    SYSCALLABLE_SYSVARS.iter().cloned()
}

fn builtins() -> Vec<(Pubkey, ProcessInstructionWithContext)> {
    let bpf_loader = solana_bpf_loader_program::solana_bpf_loader_program!();
    let upgradable_loader = solana_bpf_loader_program::solana_bpf_loader_upgradeable_program!();
    vec![
        (
            solana_sdk::system_program::id(),
            system_program::process_instruction,
        ),
        (
            solana_vote_program::id(),
            solana_vote_program::vote_instruction::process_instruction,
        ),
        (
            solana_sdk::stake::program::id(),
            solana_stake_program::stake_instruction::process_instruction,
        ),
        (
            solana_config_program::id(),
            solana_config_program::config_processor::process_instruction,
        ),
        (
            solana_sdk::secp256k1_program::id(),
            solana_secp256k1_program::process_instruction,
        ),
        (solana_sdk::bpf_loader::id(), bpf_loader.2),
        (
            solana_sdk::bpf_loader_upgradeable::id(),
            upgradable_loader.2,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use solana_program::{
        program_pack::Pack, rent::Rent, system_instruction, system_program, sysvar,
    };
    use solana_program_test::programs::spl_programs;
    use solana_sdk::account::ReadableAccount;
    use solana_sdk::{signature::Keypair, signer::Signer};
    use spl_associated_token_account::{
        create_associated_token_account, get_associated_token_address,
    };
    use spl_token::{
        instruction::{initialize_account, initialize_mint, mint_to, mint_to_checked},
        state::Mint,
    };

    use super::*;

    fn empty_account() -> Account {
        Account {
            lamports: 0,
            data: Vec::new(),
            owner: system_program::id(),
            executable: false,
            rent_epoch: 10_000,
        }
    }

    #[test]
    fn spl_token() {
        let mut accounts = HashMap::new();
        let rent = Rent::default();

        accounts.insert(
            sysvar::rent::id(),
            Account {
                lamports: 10_000,
                data: bincode::serialize(&rent).unwrap(),
                owner: system_program::id(),
                executable: false,
                rent_epoch: 10_000,
            },
        );

        init_accounts_map(&mut accounts);

        for (pubkey, account) in spl_programs(&rent) {
            accounts.insert(
                pubkey,
                Account {
                    lamports: account.lamports(),
                    data: account.data().to_vec(),
                    owner: *account.owner(),
                    executable: account.executable(),
                    rent_epoch: account.rent_epoch(),
                },
            );
        }

        let mint_keypair = Keypair::new();
        print!("mint {}", mint_keypair.pubkey());
        let mint_owner = Keypair::new();
        print!("owner {}", mint_owner.pubkey());
        let mint_owner_token_address =
            get_associated_token_address(&mint_owner.pubkey(), &mint_keypair.pubkey());
        print!("owner tok acc {}", mint_owner_token_address);
        let tok_address_init_lamports = rent.minimum_balance(spl_token::state::Account::LEN).max(1);

        accounts.insert(
            mint_owner.pubkey(),
            Account {
                lamports: 10_000_000_000,
                data: vec![],
                owner: system_program::id(),
                executable: false,
                rent_epoch: 10_000,
            },
        );

        accounts.insert(mint_keypair.pubkey(), empty_account());

        accounts.insert(mint_owner_token_address, empty_account());

        let mut ixs = Vec::new();

        let ix = system_instruction::create_account(
            &mint_owner.pubkey(),
            &mint_keypair.pubkey(),
            10_000_000,
            Mint::LEN as u64,
            &spl_token::id(),
        );
        ixs.push(ix);

        let ix = initialize_mint(
            &spl_token::id(),
            &mint_keypair.pubkey(),
            &mint_owner.pubkey(),
            None,
            2,
        );
        ixs.push(ix.unwrap());

        let ix = create_associated_token_account(
            &mint_owner.pubkey(),
            &mint_owner.pubkey(),
            &mint_keypair.pubkey(),
        );
        ixs.push(ix);

        let ix = mint_to_checked(
            &spl_token::id(),
            &mint_keypair.pubkey(),
            &mint_owner_token_address,
            &mint_owner.pubkey(),
            &[],
            42069,
            2,
        );
        ixs.push(ix.unwrap());

        let msg = Message::new(ixs.as_ref(), None);
        let message_processor = MessageProcessor::new();

        let mut tx = ProcessedMessage::new(accounts, &message_processor, msg).unwrap();

        // Create Mint account
        tx.next().unwrap().unwrap();
        let acc = tx.accounts().get(&mint_keypair.pubkey()).unwrap();
        assert_eq!(acc.lamports(), 10_000_000);
        assert_eq!(acc.owner(), &spl_token::id());
        assert_eq!(acc.data().len(), Mint::LEN);
        assert!(acc.data().iter().all(|byte| byte == &0));

        // Init Mint
        tx.next().unwrap().unwrap();
        let data = tx.accounts().get(&mint_keypair.pubkey()).unwrap().data();
        let mint = Mint::unpack(data).unwrap();
        assert_eq!(mint.mint_authority.unwrap(), mint_owner.pubkey());
        assert_eq!(mint.supply, 0);
        assert_eq!(mint.decimals, 2);
        assert_eq!(mint.freeze_authority, None.into());
        assert!(mint.is_initialized);

        // Create owner token account and init
        // This will test CPI
        tx.next().unwrap().unwrap();
        let acc = tx.accounts().get(&mint_owner_token_address).unwrap();
        assert_eq!(acc.lamports(), tok_address_init_lamports);
        assert_eq!(acc.owner(), &spl_token::id());
        assert_eq!(acc.data().len(), spl_token::state::Account::LEN);

        let data = acc.data();
        let account = spl_token::state::Account::unpack(data).unwrap();
        assert_eq!(account.mint, mint_keypair.pubkey());
        assert_eq!(account.owner, mint_owner.pubkey());
        assert_eq!(account.amount, 0);
        assert_eq!(account.delegate, None.into());
        assert_eq!(account.state, spl_token::state::AccountState::Initialized);
        assert_eq!(account.is_native, None.into());
        assert_eq!(account.delegated_amount, 0);
        assert_eq!(account.close_authority, None.into());

        let acc = tx.accounts().get(&mint_owner.pubkey()).unwrap();
        assert_eq!(
            acc.lamports(),
            10_000_000_000 - 10_000_000 - tok_address_init_lamports
        );

        // Transfer to token account
        tx.next().unwrap().unwrap();
        let data = tx.accounts().get(&mint_keypair.pubkey()).unwrap().data();
        let mint = Mint::unpack(data).unwrap();
        assert_eq!(mint.supply, 42069);

        let data = tx.accounts().get(&mint_owner_token_address).unwrap().data();

        let account = spl_token::state::Account::unpack(data).unwrap();
        assert_eq!(account.mint, mint_keypair.pubkey());
        assert_eq!(account.owner, mint_owner.pubkey());
        assert_eq!(account.amount, 42069);
    }
}
