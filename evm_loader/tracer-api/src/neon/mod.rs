mod account_storage;
mod diff;
pub mod provider;
mod tracer;
pub mod tools;

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::sync::Arc;

use anyhow::anyhow;
use tracing::{debug, info, warn};

use evm::backend::Apply;
use evm::{ExitReason, Transfer, H160, H256, U256};
use evm_loader::instruction::EvmInstruction;
use evm_loader::transaction::UnsignedTransaction;
use evm_loader::{
    executor::Machine,
    executor_state::{ExecutorState, ExecutorSubstate},
};

use evm_loader::account::{EthereumAccount, EthereumContract};

//use solana_client::rpc_client::RpcClient;
use solana_program::keccak::hash;
use solana_sdk::message::Message as SolanaMessage;

use crate::db::DbClient as RpcClient;
use crate::types::ec::pod_account::{diff_pod, PodAccount};
use crate::types::ec::state_diff::StateDiff;
use crate::types::ec::trace::{FlatTrace, FullTraceData, VMTrace};
use crate::types::TxMeta;

use account_storage::EmulatorAccountStorage;
use diff::prepare_state_diff;
use provider::{DbProvider, MapProvider, Provider};
use tracer::Tracer;
use solana_sdk::{account::Account, pubkey::Pubkey};
use std::{borrow::BorrowMut, cell::RefCell, rc::Rc};

pub enum EvmAccount<'a> {
    User(EthereumAccount<'a>),
    Contract(EthereumAccount<'a>, EthereumContract<'a>),
}

use solana_sdk::account_info::AccountInfo;
use arrayref::{array_ref};
use evm_loader::account::{ACCOUNT_SEED_VERSION};

pub trait To<T> {
    fn to(self) -> T;
}

macro_rules! impl_to {
    ($x:ty => $y:ty; $n:literal) => {
        impl To<$y> for $x {
            fn to(self) -> $y {
                let arr: [u8; $n] = self.into();
                <$y>::from(arr)
            }
        }

        impl To<$x> for $y {
            fn to(self) -> $x {
                let arr: [u8; $n] = self.into();
                <$x>::from(arr)
            }
        }
    };
}
impl_to!(U256 => ethereum_types::H256; 32);
impl_to!(U256 => ethereum_types::U256; 32);
impl_to!(H256 => ethereum_types::H256; 32);
impl_to!(H160 => ethereum_types::H160; 20);

type Error = anyhow::Error;


#[derive(Clone)]
pub struct Config {
    pub rpc_client: Arc<RpcClient>,
    //pub websocket_url: String,
    pub evm_loader: Pubkey,
    // #[allow(unused)]
    // fee_payer: Pubkey,
    //signer: Box<dyn Signer + Send>,
    //pub keypair: Option<Keypair>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            //"evm_loader={:?}, signer={:?}",
            "evm_loader={:?}",
            self.evm_loader //, self.signer
        )
    }
}

#[derive(Debug)]
pub struct TracedCall {
    pub vm_trace: Option<VMTrace>,
    pub state_diff: Option<StateDiff>,
    pub traces: Vec<FlatTrace>,
    pub full_trace_data: Vec<FullTraceData>,
    pub js_trace: Option<serde_json::Value>,
    pub result: Vec<u8>,
    pub used_gas: u64,
    pub exit_reason: ExitReason,
}

pub fn command_filter_traces(
    config: &Config,
    from_slot: Option<u64>,
    to_slot: Option<u64>,
    from_address: Option<Vec<H160>>,
    to_address: Option<Vec<H160>>,
    offset: Option<usize>,
    count: Option<usize>,
) -> Result<Vec<TxMeta<TracedCall>>, Error> {
    let transactions = config.rpc_client.get_transactions(
        from_slot,
        to_slot,
        from_address,
        to_address,
        offset,
        count,
    )?;
    debug!("{:?}", transactions);

    transactions
        .into_iter()
        .map(|tx| replay_transaction(config, tx, None))
        .filter_map(Result::transpose)
        .collect()
}

pub fn command_replay_block(config: &Config, slot: u64) -> Result<Vec<TxMeta<TracedCall>>, Error> {
    let transactions = config.rpc_client.get_transactions_by_slot(slot)?;

    transactions
        .into_iter()
        .map(|tx| replay_transaction(config, tx, None))
        .filter_map(Result::transpose)
        .collect()
}

fn get_transaction_from_holder(data: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let (header, rest) = data.split_at(1);
    if header[0] != 0 {
        // not AccountData::Empty
        return Err(anyhow!("bad account kind: {}", header[0]));
    }
    let (signature, rest) = rest.split_at(65);
    let (trx_len, rest) = rest.split_at(8);
    let trx_len = trx_len.try_into().ok().map(u64::from_le_bytes).unwrap();
    let trx_len = usize::try_from(trx_len)?;
    let (trx, _rest) = rest.split_at(trx_len as usize);

    Ok((trx, signature))
}

fn replay_transaction(
    config: &Config,
    message: TxMeta<SolanaMessage>,
    trace_code: Option<String>,
) -> Result<Option<TxMeta<TracedCall>>, Error> {
    use crate::replay;

    let (meta, message) = message.split();
    let slot = meta.slot;

    let mut traced_call = None;
    let msg2 = message.clone();

    let mut idx_to_key = HashMap::new();
    let mut account_data = HashMap::new();
    replay::init_accounts_map(&mut account_data);

    let mut accounts_to_get = message
        .account_keys
        .iter()
        .copied()
        .chain(replay::syscallable_sysvars())
        .collect::<HashSet<_>>();

    config
        .rpc_client
        .get_accounts_for_tx(meta.eth_signature)?
        .into_iter()
        .for_each(|(key, account)| {
            account_data.insert(key, account);
            accounts_to_get.remove(&key);
        });

    if !accounts_to_get.is_empty() {
        warn!(
            "did not get accounts by tx: {:?}, requesting by slot",
            accounts_to_get
        );
        config
            .rpc_client
            .get_accounts_at_slot(accounts_to_get.into_iter(), slot)?
            .into_iter()
            .for_each(|(key, account)| {
                account_data.insert(key, account);
            })
    }

    for (idx, pubkey) in message.account_keys.iter().enumerate() {
        idx_to_key.insert(idx as u8, pubkey);
    }

    info!("done fetching accounts");

    let processor = replay::MessageProcessor::new();
    let mut processed = replay::ProcessedMessage::new(account_data, &processor, msg2)?;

    for (i, instruction) in message.instructions.iter().enumerate() {
        debug!("{:?}", instruction);
        let program = idx_to_key[&instruction.program_id_index];
        match program {
            program if program == &config.evm_loader => {
                info!("instruction for neon program");
                let (tag, instruction_data) = instruction.data.split_first().unwrap();

                let evm_instruction = EvmInstruction::parse(tag)?;
                let (from, transaction) =
                    match evm_instruction {
                        EvmInstruction::CallFromRawEthereumTX => {
                            let caller = H160::from(*array_ref![instruction_data, 4, 20]);
                            let unsigned_msg = &instruction_data[4 + 20 + 65..];
                            (
                                caller,
                                rlp::decode::<UnsignedTransaction>(unsigned_msg),
                            )
                        }
                        EvmInstruction::PartialCallOrContinueFromRawEthereumTX => {
                            let caller = H160::from(*array_ref![instruction_data, 4 + 8, 20]);
                            let unsigned_msg = &instruction_data[4 + 8 + 20 + 65..];
                            (
                                caller,
                                rlp::decode::<UnsignedTransaction>(unsigned_msg),
                            )
                        }
                        //with holder
                        EvmInstruction::ExecuteTrxFromAccountDataIterativeV02 | EvmInstruction::ExecuteTrxFromAccountDataIterativeOrContinue => {
                            let key = idx_to_key.get(&instruction.accounts[0]).unwrap();
                            let holder = processed.accounts().get(key).unwrap();
                            let (trx, sign) = get_transaction_from_holder(&holder.data)?;

                            (meta.from, rlp::decode::<UnsignedTransaction>(trx))
                        }
                        _ => {
                            // TODO: handle it somehow
                            warn!("unhandled neon instruction {:?}", evm_instruction);
                            continue;
                        }
                };
                debug!("{:?}", instruction);
                debug!("{:?}", transaction);

                let transaction = transaction?;
                let provider = MapProvider::new(processed.accounts(), config.evm_loader, slot);

                let traced = command_trace_call(
                    provider,
                    transaction.to,
                    from,
                    Some(transaction.call_data),
                    Some(transaction.value),
                    Some(transaction.gas_limit.as_u64()),
                    Some(slot),
                    trace_code.clone(),
                )?;
                traced_call = Some(traced);
                continue;
            }
            program => {
                info!("program {} is not NEON, using processor", program);
                processed.process_instruction(i)?;
                continue;
            }
        }
    }

    Ok(traced_call.map(|call| meta.wrap(call)))
}

pub fn command_replay_transaction(
    config: &Config,
    transaction_hash: H256,
    trace_code: Option<String>,
) -> Result<TxMeta<TracedCall>, Error> {
    if let Some(msg) = config.rpc_client.get_transaction_data(transaction_hash)? {
        return Ok(replay_transaction(config, msg, trace_code)?.unwrap());
    }
    Err(anyhow::anyhow!(
        "transaction {} not found",
        transaction_hash
    ))
}


fn deployed_contract_id<P>(
    provider: &P,
    caller_id: &H160,
    block_number: Option<u64>) -> Result<H160, Error>
where
    P: Provider
{
    let (caller_sol, _) =  Pubkey::find_program_address(
        &[&[ACCOUNT_SEED_VERSION], caller_id.as_bytes()], provider.evm_loader(),
    );

    let mut acc = match provider.get_account_at_slot(&caller_sol, block_number.unwrap())? {
        Some(acc) => acc,
        None => return Ok(H160::default())
    };

    let info = account_info(&caller_sol, &mut acc);
    let account = EthereumAccount::from_account(provider.evm_loader(), &info)?;

    let trx_count = account.trx_count;
    let program_id = get_program_ether(caller_id, trx_count);

    Ok(program_id)
}


#[allow(clippy::too_many_lines)]
pub fn command_trace_call<P>(
    provider: P,
    contract: Option<H160>,
    caller_id: H160,
    data: Option<Vec<u8>>,
    value: Option<U256>,
    gas: Option<u64>,
    block_number: Option<u64>,
    trace_code: Option<String>,
) -> Result<TracedCall, Error>
where
    P: Provider,
{
    info!(
        "command_emulate(contract= {:?}, caller_id={:?}, data={:?}, value={:?})",
        contract,
        caller_id,
        &hex::encode(data.clone().unwrap_or_default()),
        value
    );

    let storage = EmulatorAccountStorage::new(provider, block_number);

    // u64::MAX is too large, remix gives this error:
    // Gas estimation errored with the following message (see below).
    // Number can only safely store up to 53 bits
    let gas_limit = U256::from(gas.unwrap_or(50_000_000));

    let mut executor = Machine::new(caller_id, &storage)?;
    debug!("Executor initialized");

    let js_tracer = trace_code
        .as_ref()
        .and_then(|code| Some(crate::js::JsTracer::new(code).unwrap()))
        .map(|tracer| Box::new(tracer) as Box<_>);

    let mut tracer = Tracer::new(js_tracer);

    let (_, exit_reason) = tracer.using(|| match contract {
        Some(contract_id) => {
            debug!(
                "call_begin(caller_id={:?}, contract_id={:?}, data={:?}, value={:?})",
                caller_id,
                contract_id,
                &hex::encode(data.clone().unwrap_or_default()),
                value
            );
            executor.call_begin(
                caller_id,
                contract_id,
                data.unwrap_or_default(),
                value.unwrap_or_default(),
                gas_limit,
            )?;

            match executor.execute_n_steps(100_000){
                Ok(()) => {
                    info!("too many steps");
                    return Err(anyhow!("bad account kind: "))
                },
                Err(result) => Ok(result)
            }
            // Ok::<_, solana_program::program_error::ProgramError>(executor.execute())
        }
        None => {
            let contract_id = deployed_contract_id(&provider,  &caller_id, block_number)?;
            debug!(
                "create_begin(contract_id={:?}, data={:?}, value={:?})",
                contract_id,
                &hex::encode(data.clone().unwrap_or_default()),
                value
            );
            executor.create_begin(
                contract_id,
                data.unwrap_or_default(),
                value.unwrap_or_default(),
                gas_limit,
            )?;
            match executor.execute_n_steps(100_000){
                Ok(()) => {
                    info!("too many steps");
                    return Err(anyhow!("bad account kind: "))
                },
                Err(result) => Ok(result)
            }
        }
    })?;

    let (vm_trace, traces, full_trace_data, js_trace, result) = tracer.into_traces();

    debug!(
        "Execute done, exit_reason={:?}, result={:?}, vm_trace={:?}",
        exit_reason, result, vm_trace
    );
    let used_gas = executor.used_gas().as_u64();
    let executor_state = executor.into_state();

    debug!("used_gas={:?}", used_gas);
    let applies_logs = if exit_reason.is_succeed() {
        debug!("Succeed execution");
        Some(executor_state.deconstruct())
    } else {
        None
    };

    debug!("Call done");
    let state_diff = match exit_reason {
        ExitReason::Succeed(_) => {
            let (applies,
                _logs,
                transfers,
                spl_transfers,
                spl_approves,
                withdrawals,
                erc20_approves) = applies_logs.unwrap();

            Some(prepare_state_diff(
                &storage,
                applies.clone(),
                transfers.clone(),
            ))
        }
        ExitReason::Error(_) | ExitReason::Revert(_) | ExitReason::Fatal(_) => None,
        ExitReason::StepLimitReached => unreachable!(),
    };

    info!("result: {}", &hex::encode(&result));

    if !exit_reason.is_succeed() {
        debug!("Not succeed execution");
    }

    let traced_call = TracedCall {
        vm_trace,
        state_diff,
        traces,
        full_trace_data,
        js_trace,
        result,
        used_gas,
        exit_reason,
    };

    Ok(traced_call)
}

pub fn command_trace_raw(
    config: &Config,
    transaction: Vec<u8>,
    block_number: Option<u64>,
) -> Result<TracedCall, Error> {
    use crate::types::ec::transaction::{Action, SignedTransaction, TypedTransaction};

    let tx = TypedTransaction::decode(&transaction)?;
    let tx = SignedTransaction::new(tx)?;
    info!(
        "ethereum tx signed {:?}, sender {} hash {}",
        tx,
        tx.sender(),
        tx.hash()
    );
    let caller_id = tx.sender();
    let data = &tx.unsigned.tx().data;
    let value = tx.unsigned.tx().value;
    let gas = tx.unsigned.tx().gas;
    let contract_id = match tx.unsigned.tx().action {
        Action::Call(addr) => Some(addr),
        _ => None,
    };

    let provider = DbProvider::new(config.rpc_client.clone(), config.evm_loader);

    command_trace_call(
        provider,
        contract_id.map(To::to),
        caller_id.to(),
        Some(data.clone()),
        Some(value.to()),
        Some(gas.as_u128() as u64),
        block_number,
        None,
    )
}


// fn get_ether_account_nonce<P: Provider>(
//     provider: &P,
//     caller_sol: &Pubkey,
//     slot: u64,
// ) -> Result<(u64, H160, Pubkey), Error> {
//     // let data: Vec<u8>;
//     let info=   match provider.get_account_at_slot(caller_sol, slot)? {
//         Some(acc) => AccountInfo::from(&acc),
//         None => return Ok((u64::default(), H160::default(), Pubkey::default())),
//     };
//
//     let ether_account = EthereumAccount::from_account(provider.evm_loader(), &info)
//         .unwrap_or_else(
//             // anyhow::bail!("Caller has incorrect type")
//         );
//
//     debug!("Caller: ether {}, solana {}", ether_account.ether, ether_account.info.key);
//     debug!("Caller trx_count: {} ", ether_account.trx_count);
//     debug!("caller_token = {}", ether_account.eth_token_account);
//
//     Ok((ether_account.trx_count, ether_account.ether, ether_account.eth_token_account))
// }

fn get_program_ether(caller_ether: &H160, trx_count: u64) -> H160 {
    let trx_count_256: U256 = U256::from(trx_count);
    let mut stream = rlp::RlpStream::new_list(2);
    stream.append(caller_ether);
    stream.append(&trx_count_256);
    keccak256_h256(&stream.out()).into()
}

#[must_use]
pub fn keccak256_h256(data: &[u8]) -> H256 {
    H256::from(hash(data).to_bytes())
}

/// Creates new instance of `AccountInfo` from `Account`.
pub fn account_info<'a>(key: &'a Pubkey, account: &'a mut Account) -> AccountInfo<'a> {
    AccountInfo {
        key,
        is_signer: false,
        is_writable: false,
        lamports: Rc::new(RefCell::new(&mut account.lamports)),
        data: Rc::new(RefCell::new(&mut account.data)),
        owner: &account.owner,
        executable: account.executable,
        rent_epoch: account.rent_epoch,
    }
}

