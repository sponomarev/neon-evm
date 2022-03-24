// use evm::gasometer::{tracing as gas_tracing, Snapshot};
use evm::{Capture, ExitReason, ExitSucceed, Memory, H160, H256, U256};
use evm::{Opcode, Stack};
use evm_loader::tracing as transaction_tracing;
use evm_runtime::tracing as vm_tracing;

use tracing::{debug, warn};

use crate::js;
use crate::neon::To;
use crate::types::ec::trace::{
    ActionParams, ActionType, Call, Create, ExecutiveTracer, ExecutiveVMTracer, FlatTrace,
    FullTraceData, Tracer as _, VMTrace, VMTracer as _, INSTRUCTIONS,
};

environmental::environmental!(tracer: Tracer);

pub struct Tracer {
    vm: VmTracer,
    tracer: ExecutiveTracer,
    data: Vec<FullTraceData>,
    return_value: Vec<u8>,
    js_tracer: Option<Box<dyn js::Tracer>>,
}

impl Tracer {
    pub fn new(js_tracer: Option<Box<dyn js::Tracer>>) -> Self {
        Tracer {
            vm: VmTracer::init(),
            tracer: ExecutiveTracer::default(),
            data: vec![],
            return_value: vec![],
            js_tracer,
        }
    }

    fn with_js(&mut self, f: impl FnOnce(&mut dyn js::Tracer)) {
        self.js_tracer.as_mut().map(|jst| f(&mut **jst));
    }

    pub fn using<F: FnOnce() -> R, R>(&mut self, f: F) -> R {
        tracer::using(self, || {
            struct Proxy;

            macro_rules! impl_proxy {
                ($typ: ty, $path: ident) => {
                    impl $path::EventListener for $typ {
                        fn event(&mut self, ev: $path::Event) {
                            tracer::with(|tracer| tracer.event(ev));
                        }
                    }
                };
            }

            impl_proxy!(Proxy, vm_tracing);
            impl_proxy!(Proxy, transaction_tracing);

            transaction_tracing::using(&mut Proxy, || {
                vm_tracing::using(&mut Proxy, || f())
            })
        })
    }

    pub fn into_traces(
        mut self,
    ) -> (
        Option<VMTrace>,
        Vec<FlatTrace>,
        Vec<FullTraceData>,
        Option<serde_json::Value>,
        Vec<u8>,
    ) {
        let vm = self.vm.tracer.drain();
        let traces = self.tracer.drain();
        let js_trace = self
            .js_tracer
            .as_mut()
            .and_then(|jst| jst.get_result().ok());
        (vm, traces, self.data, js_trace, self.return_value)
    }
}

impl vm_tracing::EventListener for Tracer {
    fn event(&mut self, ev: vm_tracing::Event) {
        debug!("vm event: {:?}", ev);
        if let vm_tracing::Event::Step {
            position,
            stack,
            memory,
            opcode,
            context,
            ..
        } = ev
        {
            if let Some((index, value)) = self.vm.storage_accessed.take() {
                if let Some(data) = self.data.last_mut() {
                    data.storage = Some((index, value));
                }
            }

            let stack = (0..stack.len())
                .rev()
                .map(|i| stack.peek(i).unwrap())
                .collect::<Vec<_>>();
            let memory = memory.data().to_vec();
            self.data.push(FullTraceData {
                stack: stack.clone(),
                memory: memory.clone(),
                storage: None,
            });

            let pc = position.unwrap();
            let depth = self.vm.tracer.depth as i32;
            self.with_js(move |js| {
                let stack = stack.into_iter().map(|x| x).collect();
                let contract = js::Contract {
                    address: context.address.into(),
                    caller: context.caller.into(),
                    apparent_value: Some(context.apparent_value),
                };
                let ctx = js::ScopeContext {
                    stack,
                    memory,
                    contract,
                };
                js.capture_state(pc as u64, opcode, 0, ctx, &[], depth, None);
            });
        }

        self.vm.event(ev)
    }
}

// // TODO: Make this a method of `Event`
// fn get_snapshot_from_event(event: &gas_tracing::Event) -> Snapshot {
//     use gas_tracing::Event::*;
//
//     let snapshot = match event {
//         RecordCost { snapshot, .. } => snapshot,
//         RecordRefund { snapshot, .. } => snapshot,
//         RecordStipend { snapshot, .. } => snapshot,
//         RecordDynamicCost { snapshot, .. } => snapshot,
//         RecordTransaction { snapshot, .. } => snapshot,
//     };
//     *snapshot
// }

// impl gas_tracing::EventListener for Tracer {
//     fn event(&mut self, ev: gas_tracing::Event) {
//         debug!("gas event: {:?}", ev);
//         use gas_tracing::Event::*;
//
//         let snapshot = get_snapshot_from_event(&ev);
//         self.tracer.set_snapshot(snapshot);
//
//         match ev {
//             RecordCost { cost, snapshot } => {
//                 self.vm.gas(cost, snapshot.gas());
//             }
//             RecordDynamicCost {
//                 gas_cost,
//                 memory_gas: _,
//                 snapshot,
//                 ..
//             } => {
//                 // TODO: figure out wtf is memory gas and how to handle it properly
//                 self.vm.gas(gas_cost, snapshot.gas())
//             }
//             _ => {}
//         }
//     }
// }

impl transaction_tracing::EventListener for Tracer {
    fn event(&mut self, ev: transaction_tracing::Event) {
        debug!("transaction event: {:?}", ev);
        use crate::types::ec::trace::CallType;
        use transaction_tracing::Event;

        match ev {
            Event::Call {
                code_address,
                transfer,
                input,
                target_gas,
                is_static,
                context,
            } => {
                // TODO: Is this ok?
                let (to, value) = match transfer {
                    Some(transfer) => (transfer.target, transfer.value),
                    None => (code_address, context.apparent_value),
                };

                let call_type = CallType::Call; // TODO: Add CallScheme to event

                let gas: U256 = target_gas.map_or_else(Default::default, Into::into);

                let params = Call {
                    from: context.caller, // TODO: Maybe address?
                    to: to,
                    input: From::from(input),
                    call_type,
                    value: value,
                    gas,
                };

                self.with_js(|js| {
                    js.capture_start(context.caller, to, false, input, gas, Some(value));
                });

                self.tracer.prepare_trace_call(params, 1, false);
            }
            Event::Create {
                caller,
                address,
                scheme: _,
                value,
                init_code,
                target_gas,
            } => {
                let gas = target_gas.map_or_else(Default::default, Into::into);

                let params = Create {
                    from: caller,
                    value: value,
                    gas,
                    init: From::from(init_code),
                };

                self.with_js(|js| {
                    js.capture_start(caller, address, true, &[], gas, Some(value));
                });

                // TODO: add address to create
                self.tracer.prepare_trace_create(params, address);
            }
            Event::Suicide {
                address,
                target,
                balance,
            } => {
                self.tracer
                    .trace_suicide(address, balance, target);
            }
            Event::Exit {
                reason,
                return_value,
            } => {
                self.return_value = return_value.to_vec();

                if matches!(reason, ExitReason::Succeed(ExitSucceed::Suicided)) {
                    // just skip since we traced in event
                    // ?: maybe suicide can fail?
                    return;
                }

                if matches!(reason, ExitReason::Succeed(..)) {
                    match self.tracer.last_action_type() {
                        ActionType::Call => self
                            .tracer
                            .done_trace_call(U256::zero() /* TODO */, return_value),
                        ActionType::Create => self
                            .tracer
                            .done_trace_create(U256::zero(), return_value),
                        // Must not happen
                        _ => todo!(),
                    }
                } else {
                    self.tracer.done_trace_failed(reason);
                }
            }
            Event::TransactCall {
                caller,
                address,
                value,
                data,
                gas_limit,
            } => {
                let (to, value) = (address, value);

                let call_type = CallType::Call; // TODO: Add CallScheme to event

                let params = Call {
                    from: caller, // TODO: Maybe address?
                    to: to,
                    input: From::from(data),
                    call_type,
                    value: value,
                    gas: gas_limit,
                };

                self.with_js(|js| {
                    js.capture_enter(evm::Opcode::CALL, caller, to, data, gas_limit.as_u64(), Some(value));
                });
                self.tracer.prepare_trace_call(params, 1, false);
            }
            Event::TransactCreate {
                caller,
                value,
                init_code,
                gas_limit,
                address,
            } => {
                let params = Create {
                    from: caller,
                    value: value,
                    gas: gas_limit,
                    init: From::from(init_code),
                };

                self.with_js(|js| {
                    js.capture_enter(
                        evm::Opcode::CREATE,
                        caller,
                        address,
                        init_code,
                        gas_limit.as_u64(),
                        None,
                    );
                });
                self.tracer.prepare_trace_create(params, address);
            }
            Event::TransactCreate2 {
                caller,
                value,
                init_code,
                salt,
                gas_limit,
                address,
            } => {
                let params = Create {
                    from: caller,
                    value: value,
                    gas: gas_limit.into(),
                    init: From::from(init_code),
                };

                self.with_js(|js| {
                    js.capture_enter(
                        evm::Opcode::CREATE,
                        caller,
                        address,
                        init_code,
                        gas_limit.as_u64(),
                        None,
                    );
                });

                self.tracer.prepare_trace_create(params, address);
            }
        }
    }
}

struct InstructionData {
    pc: usize,
    instruction: u8,
    mem_written: Option<(usize, usize)>,
    store_written: Option<(U256, U256)>,
}

struct PendingTrap {
    pushed: usize,
    depth: usize,
}

struct VmTracer {
    tracer: ExecutiveVMTracer,
    pushed: usize,
    current: Option<InstructionData>,
    gas: u64,
    storage_accessed: Option<(U256, U256)>,
    trap_stack: Vec<PendingTrap>,
}

impl VmTracer {
    fn init() -> Self {
        let mut tracer = ExecutiveVMTracer::toplevel();
        tracer.prepare_subtrace(&[]);

        VmTracer {
            tracer,
            pushed: 0,
            current: None,
            gas: 0,
            storage_accessed: None,
            trap_stack: Vec::new(),
        }
    }

    fn gas(&mut self, cost: u64, gas: u64) {
        if let Some(processed) = self.current.take() {
            self.tracer.trace_prepare_execute(
                processed.pc,
                processed.instruction,
                U256::from(cost),
                processed.mem_written,
                processed.store_written.map(|(a, b)| (a, b)),
            );
        }
        self.gas = gas;
    }

    fn handle_log(&self, opcode: Opcode, stack: &Stack, memory: &[u8]) {
        tracing::info!("handling log {:?}", opcode);
        let mut offset = stack.peek(0).ok();
        let mut length = stack.peek(1).ok();
        let mut topics = Vec::new();
        match opcode {
            Opcode::LOG0 => {}
            Opcode::LOG1 => {
                topics.push(stack.peek(2));
            }
            Opcode::LOG2 => {
                topics.push(stack.peek(2));
                topics.push(stack.peek(3));
            }
            Opcode::LOG3 => {
                topics.push(stack.peek(2));
                topics.push(stack.peek(3));
                topics.push(stack.peek(4));
            }
            Opcode::LOG4 => {
                topics.push(stack.peek(2));
                topics.push(stack.peek(3));
                topics.push(stack.peek(4));
                topics.push(stack.peek(5));
            }
            _ => warn!("unexpected log opcode: {:?}", opcode),
        }

        if let (Some(offset), Some(length)) = (offset, length) {
            //let offset: ethereum_types::H256 = offset.to();
            let offset = offset.as_usize();
            let length = length.as_usize();
            tracing::info!(
                "evm event {:?} @ ({}, {})",
                memory.get(offset..offset + length),
                offset,
                offset + length
            );
        }
    }

    fn take_pending_trap(&mut self) -> Option<PendingTrap> {
        if self.trap_stack.last()?.depth == self.tracer.depth {
            self.trap_stack.pop()
        } else {
            None
        }
    }

    fn handle_step_result(&mut self, stack: &Stack, mem: &Memory, pushed: usize) {
        let gas_used = U256::from(self.gas);
        let mut stack_push = vec![];
        for i in (0..pushed).rev() {
            stack_push.push(stack.peek(i).unwrap());
        }
        let mem = &mem.data();
        self.tracer.trace_executed(gas_used, &stack_push, mem);
    }
}

pub fn pushed(opcode: Opcode) -> Option<usize> {
    INSTRUCTIONS
        .get(opcode.as_usize())
        .and_then(|i| i.as_ref())
        .map(|i| i.ret)
}

fn mem_written(instruction: Opcode, stack: &Stack) -> Option<(usize, usize)> {
    let read = |pos| stack.peek(pos).unwrap().low_u64() as usize;
    let written = match instruction {
        // Core codes
        Opcode::MSTORE | Opcode::MLOAD => Some((read(0), 32)),
        Opcode::MSTORE8 => Some((read(0), 1)),
        Opcode::CALLDATACOPY | Opcode::CODECOPY => Some((read(0), read(2))),
        // External codes
        Opcode::EXTCODECOPY => Some((read(1), read(3))),
        Opcode::RETURNDATACOPY => Some((read(0), read(2))),
        Opcode::CALL | Opcode::CALLCODE => Some((read(5), read(6))),
        Opcode::DELEGATECALL | Opcode::STATICCALL => Some((read(4), read(5))),
        /* Remaining external opcodes that do not affect memory:
          Opcode::SHA3 | Opcode::ADDRESS | Opcode::BALANCE | Opcode::SELFBALANCE | Opcode::ORIGIN
        | Opcode::CALLER | Opcode::CALLVALUE | Opcode::GASPRICE | Opcode::EXTCODESIZE
        | Opcode::EXTCODEHASH | Opcode::RETURNDATASIZE | Opcode::BLOCKHASH | Opcode::COINBASE
        | Opcode::TIMESTAMP | Opcode::NUMBER | Opcode::DIFFICULTY | Opcode::GASLIMIT
        | Opcode::CHAINID | Opcode::SLOAD | Opcode::SSTORE | Opcode::GAS | Opcode::LOG0
        | Opcode::LOG1 | Opcode::LOG2 | Opcode::LOG3 | Opcode::LOG4 | Opcode::CREATE
        | Opcode::CREATE2
        */
        _ => None,
    };

    /// Checks whether offset and size is valid memory range
    fn is_valid_range(off: usize, size: usize) -> bool {
        // When size is zero we haven't actually expanded the memory
        let overflow = off.overflowing_add(size).1;
        size > 0 && !overflow
    }

    match written {
        Some((offset, size)) if !is_valid_range(offset, size) => None,
        written => written,
    }
}

fn store_written(instruction: Opcode, stack: &Stack) -> Option<(U256, U256)> {
    match instruction {
        Opcode::SSTORE => Some((stack.peek(0).unwrap(), stack.peek(1).unwrap())),
        _ => None,
    }
}

impl vm_tracing::EventListener for VmTracer {
    fn event(&mut self, ev: vm_tracing::Event) {
        use vm_tracing::Event::*;
        match ev {
            Step {
                context: _,
                opcode,
                position,
                stack,
                memory,
            } => {
                if let Some(pending_trap) = self.take_pending_trap() {
                    self.handle_step_result(stack, memory, pending_trap.pushed);
                }

                let pc = position.unwrap();
                debug!("pc = {:?}", pc);
                let instruction = opcode.0;
                let mem_written = mem_written(opcode, stack);
                let store_written = store_written(opcode, stack);
                self.current = Some(InstructionData {
                    pc,
                    instruction,
                    mem_written,
                    store_written,
                });
                if let Some(pushed_count) = pushed(opcode) {
                    self.pushed = pushed_count;
                } else {
                    warn!(opcode = ?opcode, "Unknown opcode");
                }
            }
            StepResult {
                result,
                stack,
                memory,
                ..
            } => {
                debug!("res");
                match result {
                    Ok(_) => self.handle_step_result(stack, memory, self.pushed),
                    Err(err) => {
                        match err {
                            Capture::Trap(opcode) => {
                                if matches!(*opcode, Opcode::SLOAD | Opcode::SSTORE) {
                                    return; // Handled in separate events
                                }

                                let pushed = self.pushed;
                                let depth = self.tracer.depth;
                                self.trap_stack.push(PendingTrap { pushed, depth });

                                match *opcode {
                                    Opcode::CALL
                                    | Opcode::CALLCODE
                                    | Opcode::DELEGATECALL
                                    | Opcode::STATICCALL => self.tracer.prepare_subtrace(&[]),
                                    Opcode::LOG0
                                    | Opcode::LOG1
                                    | Opcode::LOG2
                                    | Opcode::LOG3
                                    | Opcode::LOG4 => {
                                        self.handle_log(*opcode, stack, memory.data())
                                    }
                                    _ => (),
                                }

                                return;
                            }
                            Capture::Exit(err) => {
                                tracing::info!("exit with {:?}", err);
                                match err {
                                    // RETURN, STOP as SUICIDE opcodes
                                    ExitReason::Succeed(success) => {
                                        self.tracer.trace_executed(U256::zero(), &[], &[])
                                    }
                                    ExitReason::Error(_)
                                    | ExitReason::Fatal(_)
                                    | ExitReason::Revert(_)
                                    | ExitReason::StepLimitReached => self.tracer.trace_failed(),
                                }
                                self.tracer.done_subtrace();
                            }
                        }
                        self.pushed = 0;
                    }
                }
            }
            SLoad {
                address: _,
                index,
                value,
            } => {
                self.storage_accessed = Some((index, value));
                self.tracer
                    .trace_executed(U256::zero(), &[value], &[]);

                //println!("sload called {} {} {}", address, index, value);
            }
            SStore {
                address,
                index,
                value,
            } => {
                self.storage_accessed = Some((index, value));
                self.tracer.trace_executed(U256::zero(), &[], &[]);
                /* TODO */
            }
        }
    }
}
