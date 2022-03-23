use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use thiserror::Error;

use solana_program::bpf_loader_upgradeable::{self, UpgradeableLoaderState};
use solana_program::feature;
use solana_program::{
    hash::Hash,
    instruction::{CompiledInstruction, Instruction, InstructionError},
    message::Message,
    pubkey::Pubkey,
};
use solana_runtime::message_processor::{self, Executors};
use solana_sdk::account::ReadableAccount;
use solana_sdk::account_utils::StateMut;
use solana_sdk::feature_set::{self, /* remove_native_loader, */ FeatureSet};
use solana_sdk::{
    account::{Account, AccountSharedData},
    keyed_account::{create_keyed_accounts_unified, KeyedAccount},
    process_instruction::{
        BpfComputeBudget, ComputeMeter, Executor, InvokeContext, InvokeContextStackFrame, Logger,
        ProcessInstructionWithContext,
    },
    transaction::TransactionError,
};

use super::builtins;
use super::native_loader::NativeLoader;

fn create_keyed_accounts<'a>(
    message: &'a Message,
    instruction: &'a CompiledInstruction,
    executable_accounts: &'a [(Pubkey, Rc<RefCell<AccountSharedData>>)],
    accounts: &'a [(Pubkey, Rc<RefCell<AccountSharedData>>)],
) -> Vec<(bool, bool, &'a Pubkey, &'a RefCell<AccountSharedData>)> {
    executable_accounts
        .iter()
        .map(|(key, account)| (false, false, key, account as &RefCell<AccountSharedData>))
        .chain(instruction.accounts.iter().map(|index| {
            let index = *index as usize;
            (
                message.is_signer(index),
                message.is_writable(index),
                &accounts[index].0,
                &accounts[index].1 as &RefCell<AccountSharedData>,
            )
        }))
        .collect::<Vec<_>>()
}

#[derive(Debug, Error)]
#[error("account not found {0}")]
pub struct AccountNotFound(Pubkey);

#[derive(Debug, Error)]
pub enum Error {
    #[error("instruction error: {0}")]
    Instruction(#[from] InstructionError),

    #[error("{0}")]
    AccountNotFound(#[from] AccountNotFound),

    #[error("invalid transaction")]
    InvalidTrasaction(#[from] TransactionError),
}

pub struct MessageProcessor {
    builtins: Vec<(Pubkey, ProcessInstructionWithContext)>,
    native_loader: NativeLoader,
    feature_set: FeatureSet,
}

impl MessageProcessor {
    pub fn new() -> Self {
        Self {
            builtins: builtins(),
            native_loader: NativeLoader::default(),
            feature_set: FeatureSet::default(),
        }
    }

    fn process_instruction(
        &self,
        program_id: &Pubkey,
        data: &[u8],
        invoke_context: &mut dyn InvokeContext,
    ) -> Result<(), InstructionError> {
        if let Some(root_account) = invoke_context.get_keyed_accounts()?.iter().next() {
            let root_id = root_account.unsigned_key();
            if solana_sdk::native_loader::check_id(&root_account.owner()?) {
                for (id, process_instruction) in &self.builtins {
                    if id == root_id {
                        invoke_context.remove_first_keyed_account()?;
                        // Call the builtin program
                        return process_instruction(program_id, data, invoke_context);
                    }
                }
                // TODO: Native loader
                if true {
                    // !invoke_context.is_feature_active(&remove_native_loader::id()) {
                    // Call the program via the native loader
                    println!("native call");
                    return self.native_loader.process_instruction(
                        &solana_sdk::native_loader::id(),
                        data,
                        invoke_context,
                    );
                }
            } else {
                let owner_id = &root_account.owner()?;
                for (id, process_instruction) in &self.builtins {
                    if id == owner_id {
                        // Call the program via a builtin loader
                        return process_instruction(program_id, data, invoke_context);
                    }
                }
            }
        }
        Err(InstructionError::UnsupportedProgramId)
    }
}

pub struct ProcessedMessage<'a> {
    message_processor: &'a MessageProcessor,
    loaders: Vec<Vec<(Pubkey, Rc<RefCell<AccountSharedData>>)>>,
    accounts: Vec<(Pubkey, Rc<RefCell<AccountSharedData>>)>,
    all_accounts: HashMap<Pubkey, Account>,
    message: Message,
    current_idx: usize,
    exited: bool,
}

impl<'a> ProcessedMessage<'a> {
    pub fn new(
        accounts: HashMap<Pubkey, Account>,
        message_processor: &'a MessageProcessor,
        message: Message,
    ) -> Result<Self, Error> {
        let mut cache = HashMap::new();
        let mut loaders = Vec::new();
        let mut accounts_vec = Vec::new();

        for (idx, account_key) in message.account_keys.iter().enumerate() {
            if message.is_non_loader_key(account_key, idx) {
                let acc = Self::load(&accounts, &mut cache, account_key)?;
                accounts_vec.push((*account_key, acc));
            }
        }

        for ix in message.instructions.iter() {
            let program_id = ix.program_id(&message.account_keys);
            let loaders_inner = Self::get_loaders(&accounts, &mut cache, program_id)?;
            loaders.push(loaders_inner);
        }

        Ok(Self {
            message_processor,
            loaders,
            accounts: accounts_vec,
            all_accounts: accounts,
            message,
            current_idx: 0,
            exited: false,
        })
    }

    pub fn accounts(&self) -> &HashMap<Pubkey, Account> {
        &self.all_accounts
    }

    // ===== Private methods =====

    fn load(
        accounts: &HashMap<Pubkey, Account>,
        cache: &mut HashMap<Pubkey, Rc<RefCell<AccountSharedData>>>,
        key: &Pubkey,
    ) -> Result<Rc<RefCell<AccountSharedData>>, AccountNotFound> {
        match cache.get(key) {
            Some(account) => Ok(account.clone()),
            None => accounts
                .get(key)
                .cloned()
                .map(Into::into)
                .map(RefCell::new)
                .map(Rc::new)
                .map(|acc| {
                    cache.insert(*key, acc.clone());
                    acc
                })
                .ok_or_else(|| AccountNotFound(*key)),
        }
    }

    fn get_loaders(
        all_accounts: &HashMap<Pubkey, Account>,
        cache: &mut HashMap<Pubkey, Rc<RefCell<AccountSharedData>>>,
        program_id: &Pubkey,
    ) -> Result<Vec<(Pubkey, Rc<RefCell<AccountSharedData>>)>, Error> {
        let mut accounts = Vec::new();
        let mut depth = 0;
        let mut program_id = *program_id;

        loop {
            if solana_sdk::native_loader::check_id(&program_id) {
                // At the root of the chain, ready to dispatch
                break;
            }

            if depth >= 5 {
                panic!("call chain too deep");
            }
            depth += 1;

            let program = Self::load(all_accounts, cache, &program_id)?;

            if !RefCell::borrow(&program).executable() {
                return Err(TransactionError::InvalidProgramForExecution.into());
            }

            // Add loader to chain
            let program_owner = *RefCell::borrow(&program).owner();

            if bpf_loader_upgradeable::check_id(&program_owner) {
                let program = RefCell::borrow(&program);

                // The upgradeable loader requires the derived ProgramData account
                if let Ok(UpgradeableLoaderState::Program {
                    programdata_address,
                }) = program.state()
                {
                    let program = Self::load(all_accounts, cache, &program_id)?;
                    accounts.insert(0, (programdata_address, program));
                } else {
                    panic!("{:?}", TransactionError::InvalidProgramForExecution);
                }
            }

            accounts.insert(0, (program_id, program));
            program_id = program_owner;
        }

        Ok(accounts)
    }

    pub fn process_instruction(&mut self, idx: usize) -> Result<(), InstructionError> {
        let instruction = &self.message.instructions[idx];
        let executable_accounts = &self.loaders[idx];
        let program_id = instruction.program_id(&self.message.account_keys);
        let keyed_accounts = create_keyed_accounts(
            &self.message,
            instruction,
            executable_accounts,
            &self.accounts,
        );
        let compute_budget = BpfComputeBudget::default();

        let mut invoke_context = LightIC {
            instruction_index: idx,
            invoke_stack: Vec::new(),
            accounts: &self.accounts,
            programs: &self.message_processor.builtins,
            blockhash: Hash::default(),
            compute_budget,
            all_accounts: &self.all_accounts,
            executors: Rc::new(RefCell::new(Executors::default())),
            feature_set: &self.message_processor.feature_set,
        };

        invoke_context
            .invoke_stack
            .push(InvokeContextStackFrame::new(
                *program_id,
                create_keyed_accounts_unified(&keyed_accounts),
            ));

        self.message_processor.process_instruction(
            program_id,
            &instruction.data,
            &mut invoke_context,
        )?;
        self.update();
        Ok(())
    }

    fn update(&mut self) {
        for (key, account) in &self.accounts {
            let account = RefCell::borrow(account);

            self.all_accounts.insert(
                *key,
                Account {
                    lamports: account.lamports(),
                    data: account.data().to_vec(),
                    owner: *account.owner(),
                    executable: account.executable(),
                    rent_epoch: account.rent_epoch(),
                },
            );
        }
    }
}

impl<'a> Iterator for ProcessedMessage<'a> {
    type Item = Result<(), InstructionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.exited && self.current_idx < self.message.instructions.len() {
            let idx = self.current_idx;
            self.current_idx += 1;
            let result = self.process_instruction(idx);
            if result.is_err() {
                self.exited = true;
            }

            Some(result)
        } else {
            None
        }
    }
}

struct Dummy;

impl Logger for Dummy {
    fn log_enabled(&self) -> bool {
        true
    }

    fn log(&self, message: &str) {
        println!("{}", message);
    }
}

impl ComputeMeter for Dummy {
    fn consume(&mut self, amount: u64) -> Result<(), InstructionError> {
        Ok(())
    }

    fn get_remaining(&self) -> u64 {
        u64::MAX
    }
}

struct LightIC<'a> {
    instruction_index: usize,
    accounts: &'a [(Pubkey, Rc<RefCell<AccountSharedData>>)],
    programs: &'a [(Pubkey, ProcessInstructionWithContext)],

    invoke_stack: Vec<InvokeContextStackFrame<'a>>,
    compute_budget: BpfComputeBudget,
    all_accounts: &'a HashMap<Pubkey, Account>,
    executors: Rc<RefCell<Executors>>,

    blockhash: Hash,
    feature_set: &'a FeatureSet,
}

impl<'a> InvokeContext for LightIC<'a> {
    fn push(
        &mut self,
        key: &Pubkey,
        keyed_accounts: &[(bool, bool, &Pubkey, &RefCell<AccountSharedData>)],
    ) -> Result<(), InstructionError> {
        if self.invoke_stack.len() > self.compute_budget.max_invoke_depth {
            return Err(InstructionError::CallDepth);
        }

        let contains = self.invoke_stack.iter().any(|frame| frame.key == *key);
        let is_last = if let Some(last_frame) = self.invoke_stack.last() {
            last_frame.key == *key
        } else {
            false
        };
        if contains && !is_last {
            // Reentrancy not allowed unless caller is calling itself
            return Err(InstructionError::ReentrancyNotAllowed);
        }

        // Alias the keys and account references in the provided keyed_accounts
        // with the ones already existing in self, so that the lifetime 'a matches.
        fn transmute_lifetime<'a, 'b, T: Sized>(value: &'a T) -> &'b T {
            unsafe { std::mem::transmute(value) }
        }
        let keyed_accounts = keyed_accounts
            .iter()
            .map(|(is_signer, is_writable, search_key, account)| {
                self.accounts
                    .iter()
                    .position(|(key, _account)| key == *search_key)
                    .map(|index| {
                        // TODO
                        // Currently we are constructing new accounts on the stack
                        // before calling MessageProcessor::process_cross_program_instruction
                        // Ideally we would recycle the existing accounts here.
                        (
                            *is_signer,
                            *is_writable,
                            &self.accounts[index].0,
                            // &self.accounts[index] as &RefCell<AccountSharedData>
                            transmute_lifetime(*account),
                        )
                    })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(InstructionError::InvalidArgument)?;
        self.invoke_stack.push(InvokeContextStackFrame::new(
            *key,
            create_keyed_accounts_unified(keyed_accounts.as_slice()),
        ));
        Ok(())
    }

    fn pop(&mut self) {
        self.invoke_stack.pop();
    }

    fn invoke_depth(&self) -> usize {
        self.invoke_stack.len()
    }

    fn verify_and_update(
        &mut self,
        instruction: &CompiledInstruction,
        accounts: &[(Pubkey, Rc<RefCell<AccountSharedData>>)],
        write_privileges: &[bool],
    ) -> Result<(), InstructionError> {
        // TODO!: As we only running transactions that are already part of the ledger
        // TODO!: seems like there's no point in checking runtime stuff.
        // !: As for updating the account map - it's done after each instruction in ProcessedMessage

        Ok(())
    }

    fn get_caller(&self) -> Result<&Pubkey, InstructionError> {
        self.invoke_stack
            .last()
            .map(|frame| &frame.key)
            .ok_or(InstructionError::CallDepth)
    }

    fn remove_first_keyed_account(&mut self) -> Result<(), InstructionError> {
        if true
        /* !self.is_feature_active(&remove_native_loader::id()) */
        {
            let stack_frame = &mut self
                .invoke_stack
                .last_mut()
                .ok_or(InstructionError::CallDepth)?;
            stack_frame.keyed_accounts_range.start =
                stack_frame.keyed_accounts_range.start.saturating_add(1);
        }
        Ok(())
    }

    fn get_keyed_accounts(&self) -> Result<&[KeyedAccount], InstructionError> {
        self.invoke_stack
            .last()
            .map(|frame| &frame.keyed_accounts[frame.keyed_accounts_range.clone()])
            .ok_or(InstructionError::CallDepth)
    }

    fn get_programs(&self) -> &[(Pubkey, ProcessInstructionWithContext)] {
        self.programs
    }

    fn get_logger(&self) -> Rc<RefCell<dyn Logger>> {
        Rc::new(RefCell::new(Dummy))
    }

    fn get_bpf_compute_budget(&self) -> &BpfComputeBudget {
        &self.compute_budget
    }

    fn get_compute_meter(&self) -> Rc<RefCell<dyn ComputeMeter>> {
        Rc::new(RefCell::new(Dummy))
    }

    fn add_executor(&self, pubkey: &Pubkey, executor: Arc<dyn Executor>) {}

    fn update_executor(&self, pubkey: &Pubkey, executor: Arc<dyn Executor>)  {}

    fn get_executor(&self, pubkey: &Pubkey) -> Option<Arc<dyn Executor>> {
        None
    }

    fn record_instruction(&self, instruction: &Instruction) {}

    fn is_feature_active(&self, feature_id: &Pubkey) -> bool {
        self.feature_set.is_active(feature_id)
    }

    fn get_account(&self, pubkey: &Pubkey) -> Option<Rc<RefCell<AccountSharedData>>> {
        for (index, (key, account)) in self.accounts.iter().enumerate().rev() {
            if key == pubkey {
                return Some(account.clone());
            }
        }
        None
    }

    fn update_timing(&mut self, _: u64, _: u64, _: u64, _: u64) {}

    fn get_sysvar_data(&self, id: &Pubkey) -> Option<Rc<Vec<u8>>> {
        self.all_accounts
            .get(id)
            .map(|acc| Rc::new(acc.data.clone()))
    }

    fn set_return_data(&mut self, return_data: Option<(Pubkey, Vec<u8>)>) {}

    fn get_return_data(&self) -> &Option<(Pubkey, Vec<u8>)> {}

}
