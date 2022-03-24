#![allow(unused)]

use std::str::FromStr;
use std::sync::Arc;

use secret_value::Secret;
use structopt::StructOpt;
use tracing::{info, instrument, span, Level};
use tracing_subscriber::{fmt, EnvFilter};

use jsonrpsee::http_server::{HttpServerBuilder, RpcModule};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::error::{CallError, Error};
use types::TxMeta;
//use jsonrpsee::types::{async_trait, error::Error};
//
//use crate::types::ec::trace::FullTraceData;
use crate::neon::provider::DbProvider;
use crate::v1::geth::types::trace as geth;
use crate::v1::types::{
    BlockNumber, Bytes, CallRequest, Index, LocalizedTrace, TraceFilter, TraceOptions,
    TraceResults, TraceResultsWithTransactionHash,
};
use evm::H256;

type Result<T> = std::result::Result<T, Error>;

mod db;
mod js;
mod neon;
mod replay;
mod types;
mod utils;
mod v1;

#[derive(Debug, StructOpt)]
struct Options {
    #[structopt(short = "l", long = "listen", default_value = "127.0.0.1:8080")]
    addr: String,
    #[structopt(short = "c", long = "db-addr", default_value = "127.0.0.1:8123")]
    ch_addr: String,
    #[structopt(short = "p", long = "ch-password", parse(try_from_str = parse_secret))]
    ch_password: Option<Secret<String>>,
    #[structopt(short = "u", long = "ch-user")]
    ch_user: Option<String>,
    #[structopt(short = "d", long = "ch-database")]
    ch_database: Option<String>,
    #[structopt(long = "evm-loader")]
    evm_loader: solana_sdk::pubkey::Pubkey,
}

fn parse_secret<T: FromStr>(input: &str) -> std::result::Result<Secret<T>, T::Err> {
    T::from_str(input).map(Secret::from)
}

struct ParsedTraceOptions(u8);

impl ParsedTraceOptions {
    const VMTRACE: u8 = 1;
    const TRACE: u8 = 2;
    const STATE_DIFF: u8 = 4;

    fn parse(options: &[String]) -> Self {
        let mut options_set = 0;
        for option in options {
            match option.as_str() {
                "vmTrace" => {
                    options_set |= ParsedTraceOptions::VMTRACE;
                }
                "trace" => {
                    options_set |= ParsedTraceOptions::TRACE;
                }
                "stateDiff" => {
                    options_set |= ParsedTraceOptions::STATE_DIFF;
                }
                _ => {}
            }
        }
        ParsedTraceOptions(options_set)
    }

    fn vmtrace_enabled(&self) -> bool {
        self.0 & ParsedTraceOptions::VMTRACE != 0
    }

    fn trace_enabled(&self) -> bool {
        self.0 & ParsedTraceOptions::TRACE != 0
    }

    fn state_diff_enabled(&self) -> bool {
        self.0 & ParsedTraceOptions::STATE_DIFF != 0
    }
}

#[rpc(server)]
pub trait GethTrace {
    /// Returns all traces produced at given block.
    #[method(name = "debug_traceBlockByNumber")]
    fn trace_block(&self, b: geth::BlockNumber) -> Result<Option<Vec<geth::TraceResult>>>;

    /// Returns all traces of given transaction.
    #[method(name = "debug_traceTransaction")]
    fn trace_transaction(
        &self,
        t: H256,
        o: Option<geth::TraceTransactionOptions>,
    ) -> Result<Option<geth::Trace>>;

    /// Executes the given call at given block and returns the structured logs created during the execution of EVM
    #[method(name = "debug_traceCall")]
    fn trace_call(
        &self,
        a: geth::TransactionArgs,
        b: geth::BlockNumber,
        o: Option<geth::TraceTransactionOptions>,
    ) -> Result<geth::Trace>;
}

#[rpc(server)]
pub trait OpenEthereumTraces {
    /// Returns traces matching given filter.
    #[method(name = "trace_filter")]
    fn filter(&self, f: TraceFilter) -> Result<Option<Vec<LocalizedTrace>>>;

    /// Returns transaction trace at given index.
    #[method(name = "trace_get")]
    fn trace(&self, t: H256, i: Vec<Index>) -> Result<Option<LocalizedTrace>>;

    /// Returns all traces of given transaction.
    #[method(name = "trace_transaction")]
    fn transaction_traces(&self, t: H256) -> Result<Option<Vec<LocalizedTrace>>>;

    /// Returns all traces produced at given block.
    #[method(name = "trace_block")]
    fn block_traces(&self, b: BlockNumber) -> Result<Option<Vec<LocalizedTrace>>>;

    /// Executes the given call and returns a number of possible traces for it.
    #[method(name = "trace_call")]
    fn call(&self, r: CallRequest, o: TraceOptions, b: Option<BlockNumber>)
        -> Result<TraceResults>;

    /// Executes all given calls and returns a number of possible traces for each of it.
    #[method(name = "trace_callMany")]
    fn call_many(
        &self,
        rs: Vec<(CallRequest, TraceOptions)>,
        b: Option<BlockNumber>,
    ) -> Result<Vec<TraceResults>>;

    /// Executes the given raw transaction and returns a number of possible traces for it.
    #[method(name = "trace_rawTransaction")]
    fn raw_transaction(
        &self,
        b: Bytes,
        o: TraceOptions,
        bn: Option<BlockNumber>,
    ) -> Result<TraceResults>;

    /// Executes the transaction with the given hash and returns a number of possible traces for it.
    #[method(name = "trace_replayTransaction")]
    fn replay_transaction(&self, t: H256, o: TraceOptions) -> Result<TraceResults>;

    /// Executes all the transactions at the given block and returns a number of possible traces for each transaction.
    #[method(name = "trace_replayBlockTransactions")]
    fn replay_block_transactions(
        &self,
        bn: BlockNumber,
        o: TraceOptions,
    ) -> Result<Vec<TraceResultsWithTransactionHash>>;
}

fn trace_with_options(traced_call: neon::TracedCall, options: &ParsedTraceOptions) -> TraceResults {
    let neon::TracedCall {
        vm_trace,
        traces,
        state_diff,
        result,
        ..
    } = traced_call;
    TraceResults {
        vm_trace: options
            .vmtrace_enabled()
            .then(|| vm_trace.map(Into::into))
            .flatten(),
        trace: options
            .trace_enabled()
            .then(|| traces.into_iter().map(Into::into).collect())
            .unwrap_or_default(),
        state_diff: options
            .state_diff_enabled()
            .then(|| state_diff.map(Into::into))
            .flatten(),
        output: result.into(),
    }
}

#[derive(Debug, Clone)]
pub struct ServerImpl {
    neon_config: neon::Config,
}

impl ServerImpl {
    fn get_slot_by_block(&self, bn: BlockNumber) -> Option<u64> {
        match bn {
            BlockNumber::Num(num) => Some(num),
            BlockNumber::Latest => None,
            _ => todo!(),
        }
    }
}

impl GethTraceServer for ServerImpl {
    #[instrument]
    fn trace_block(&self, b: geth::BlockNumber) -> Result<Option<Vec<geth::TraceResult>>> {
        let slot = b;
        let options = geth::TraceTransactionOptions::default();
        let traced_calls = neon::command_replay_block(&self.neon_config, slot.into())?;

        Ok(Some(
            traced_calls
                .into_iter()
                .map(TxMeta::split)
                .map(|(_, call)| geth::ExecutionResult::new(call, &options))
                .map(geth::TraceResult::Result)
                .collect(),
        ))
    }

    #[instrument]
    fn trace_transaction(
        &self,
        t: H256,
        o: Option<geth::TraceTransactionOptions>,
    ) -> Result<Option<geth::Trace>> {
        use neon::To;

        let o = o.unwrap_or_default();
        let trace_code = o.tracer.clone();
        let (_meta, traced_call) =
            neon::command_replay_transaction(&self.neon_config, t, trace_code)?.split();
        if let Some(js_trace) = traced_call.js_trace {
            Ok(Some(geth::Trace::JsTrace(js_trace)))
        } else {
            Ok(Some(geth::Trace::Logs(geth::ExecutionResult::new(
                traced_call,
                &o,
            ))))
        }
    }

    #[instrument]
    fn trace_call(
        &self,
        a: geth::TransactionArgs,
        b: geth::BlockNumber,
        o: Option<geth::TraceTransactionOptions>,
    ) -> Result<geth::Trace> {
        use neon::To;
        let o = o.unwrap_or_default();
        let provider = DbProvider::new(
            self.neon_config.rpc_client.clone(),
            self.neon_config.evm_loader,
        );
        let trace_code = o.tracer.clone();

        let traced_call = neon::command_trace_call(
            provider,
            a.to,
            a.from.unwrap(), // TODO
            a.input.map(Into::into),
            a.value,
            a.gas.map(|gas| gas.as_u64()),
            Some(b.into()),
            trace_code,
        )?;
        if let Some(js_trace) = traced_call.js_trace {
            Ok(geth::Trace::JsTrace(js_trace))
        } else {
            Ok(geth::Trace::Logs(geth::ExecutionResult::new(
                traced_call,
                &o,
            )))
        }
    }
}

impl OpenEthereumTracesServer for ServerImpl {
    /// Returns traces matching given filter.
    #[instrument]
    fn filter(&self, f: TraceFilter) -> Result<Option<Vec<LocalizedTrace>>> {
        use neon::To;
        use types::ec::trace::LocalizedTrace;

        let from_slot = f
            .from_block
            .map(|block| self.get_slot_by_block(block))
            .flatten();
        let to_slot = f
            .to_block
            .map(|block| self.get_slot_by_block(block))
            .flatten();
        let from_address = f.from_address.map(|f| f.into_iter().collect());
        let to_address = f.to_address.map(|f| f.into_iter().collect());
        let offset = f.after;
        let count = f.count;
        let traced_calls = neon::command_filter_traces(
            &self.neon_config,
            from_slot,
            to_slot,
            from_address,
            to_address,
            offset,
            count,
        )
        .map_err(CallError::Failed)?;
        let traces = traced_calls
            .into_iter()
            .map(TxMeta::split)
            .map(|(meta, traced_call)| {
                traced_call.traces.into_iter().map(move |flat| {
                    LocalizedTrace {
                        action: flat.action,
                        result: flat.result,
                        subtraces: flat.subtraces,
                        trace_address: flat.trace_address,
                        transaction_number: None, // TODO: add idx to db or just enumerate?
                        transaction_hash: Some(meta.eth_signature),
                        block_number: meta.slot,
                        block_hash: H256::from_low_u64_ne(meta.slot), // TODO: revise this
                    }
                    .into()
                })
            })
            .flatten()
            .collect();
        Ok(Some(traces))
    }

    /// Returns transaction trace at given index.
    #[instrument]
    fn trace(&self, t: H256, i: Vec<Index>) -> Result<Option<LocalizedTrace>> {
        use neon::To;
        use types::ec::trace::LocalizedTrace;
        let (meta, traced_call) =
            neon::command_replay_transaction(&self.neon_config, t, None)?.split();

        // TODO: it's unclear what's index
        let trace = traced_call.traces.get(i[0].value()).map(|flat| {
            LocalizedTrace {
                action: flat.action.clone(), // TODO: remove clones
                result: flat.result.clone(),
                subtraces: flat.subtraces,
                trace_address: flat.trace_address.clone(),
                transaction_number: None, // TODO??
                transaction_hash: Some(t),
                block_number: meta.slot,
                block_hash: H256::from_low_u64_ne(meta.slot), // TODO
            }
            .into()
        });
        Ok(trace)
    }

    /// Returns all traces of given transaction.
    fn transaction_traces(&self, t: H256) -> Result<Option<Vec<LocalizedTrace>>> {
        use neon::To;
        use types::ec::trace::LocalizedTrace;

        let traced_call = neon::command_replay_transaction(&self.neon_config, t, None)?;
        let (meta, traced_call) = traced_call.split();
        let traces = traced_call
            .traces
            .into_iter()
            .map(|flat| {
                LocalizedTrace {
                    action: flat.action,
                    result: flat.result,
                    subtraces: flat.subtraces,
                    trace_address: flat.trace_address,
                    transaction_number: None, // TODO??
                    transaction_hash: Some(meta.eth_signature),
                    block_number: meta.slot,
                    block_hash: H256::from_low_u64_ne(meta.slot),
                }
                .into()
            })
            .collect();
        Ok(Some(traces))
    }

    /// Returns all traces produced at given block.
    #[instrument]
    fn block_traces(&self, b: BlockNumber) -> Result<Option<Vec<LocalizedTrace>>> {
        use neon::To;
        use types::ec::trace::LocalizedTrace;

        let slot = self.get_slot_by_block(b).unwrap(); // TODO
        let traces = neon::command_replay_block(&self.neon_config, slot)?;
        let traces = traces
            .into_iter()
            .map(TxMeta::split)
            .enumerate()
            .map(|(idx, (meta, call))| {
                call.traces.into_iter().map(move |flat| {
                    LocalizedTrace {
                        action: flat.action.into(),
                        result: flat.result.into(),
                        subtraces: flat.subtraces,
                        trace_address: flat.trace_address,
                        // !: Since we tracing whole block it's ok to use trace index.
                        // !: Anyway this must be revised if tx index hits the db schema.
                        transaction_number: Some(idx),
                        transaction_hash: Some(meta.eth_signature),
                        block_number: meta.slot,
                        block_hash: H256::from_low_u64_ne(meta.slot), // TODO
                    }
                    .into()
                })
            })
            .flatten()
            .collect();
        Ok(Some(traces))
    }

    /// Executes the given call and returns a number of possible traces for it.
    #[instrument]
    fn call(
        &self,
        req: CallRequest,
        options: TraceOptions,
        block: Option<BlockNumber>,
    ) -> Result<TraceResults> {
        use neon::To;
        let provider = DbProvider::new(
            self.neon_config.rpc_client.clone(),
            self.neon_config.evm_loader,
        );
        let traced_call = neon::command_trace_call(
            provider,
            req.to,
            req.from.unwrap(), // todo
            req.data.map(Into::into),      // todo
            req.value,
            req.gas.map(|gas| gas.as_u64()),
            block.map(|block| self.get_slot_by_block(block)).flatten(),
            None,
        )?;
        let options = ParsedTraceOptions::parse(&options);
        Ok(trace_with_options(traced_call, &options))
    }

    /// Executes all given calls and returns a number of possible traces for each of it.
    #[instrument]
    fn call_many(
        &self,
        rs: Vec<(CallRequest, TraceOptions)>,
        b: Option<BlockNumber>,
    ) -> Result<Vec<TraceResults>> {
        rs.into_iter()
            .map(|(r, o)| self.call(r, o, b.clone()))
            .collect()
    }

    /// Executes the given raw transaction and returns a number of possible traces for it.
    #[instrument]
    fn raw_transaction(
        &self,
        b: Bytes,
        options: TraceOptions,
        bn: Option<BlockNumber>,
    ) -> Result<TraceResults> {
        let slot = bn.map(|bn| self.get_slot_by_block(bn)).flatten();
        let traced_call = neon::command_trace_raw(&self.neon_config, b.into_vec(), slot)?;
        let options = ParsedTraceOptions::parse(&options);

        Ok(trace_with_options(traced_call, &options))
    }

    /// Executes the transaction with the given hash and returns a number of possible traces for it.
    #[instrument]
    fn replay_transaction(&self, t: H256, options: TraceOptions) -> Result<TraceResults> {
        use neon::To;
        let traced_call = neon::command_replay_transaction(&self.neon_config, t, None)?;
        let options = ParsedTraceOptions::parse(&options);

        Ok(trace_with_options(traced_call.value, &options))
    }

    /// Executes all the transactions at the given block and returns a number of possible traces for each transaction.
    #[instrument]
    fn replay_block_transactions(
        &self,
        bn: BlockNumber,
        options: TraceOptions,
    ) -> Result<Vec<TraceResultsWithTransactionHash>> {
        use neon::To;
        let slot = self.get_slot_by_block(bn).unwrap();
        let options = ParsedTraceOptions::parse(&options);
        let traced_calls = neon::command_replay_block(&self.neon_config, slot)?;

        Ok(traced_calls
            .into_iter()
            .map(TxMeta::split)
            .map(|(meta, call)| {
                let trace_result = trace_with_options(call, &options);
                TraceResultsWithTransactionHash {
                    output: trace_result.output,
                    trace: trace_result.trace,
                    vm_trace: trace_result.vm_trace,
                    state_diff: trace_result.state_diff,
                    transaction_hash: meta.eth_signature,
                }
            })
            .collect())
    }
}

fn init_logs() {
    let writer = || std::io::stdout();
    let subscriber = fmt::Subscriber::builder()
        .with_writer(writer)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    tracing_log::LogTracer::init().unwrap();
}

#[tokio::main]
async fn main() {
    use crate::db::DbClient;
    use std::str::FromStr;

    let options = Options::from_args();

    init_logs();

    info!(?options, "starting");

    let server = HttpServerBuilder::default()
        .build(options.addr.parse().unwrap())
        .unwrap();

    let mut client = DbClient::new(
        options.ch_addr,
        options.ch_user,
        options.ch_password.map(Secret::inner),
        options.ch_database,
    );

    let serv_impl = ServerImpl {
        neon_config: neon::Config {
            evm_loader: options.evm_loader,
            rpc_client: Arc::new(client),
        },
    };

    let mut module = RpcModule::new(());
    module.merge(OpenEthereumTracesServer::into_rpc(serv_impl.clone()));
    module.merge(GethTraceServer::into_rpc(serv_impl));

    let _handle = server.start(module).unwrap();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
