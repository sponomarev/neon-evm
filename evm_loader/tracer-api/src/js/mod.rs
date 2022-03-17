use std::cell::RefCell;
use std::rc::Rc;

use dukt::value::{PeekValue, PushValue};
use dukt::Context;
use dukt::{dukt, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use tracing::{debug, info, instrument};

use evm::{H160, H256, U256};

pub struct ScopeContext {
    pub stack: Vec<U256>,
    pub memory: Vec<u8>,
    pub contract: Contract,
}

// EvmLogger interface from geth
pub trait EvmLogger {
    /// initialize tracing operation
    fn capture_start(
        &mut self,
        from: evm::H160,
        to: evm::H160,
        create: bool,
        input: &[u8],
        gas: evm::U256,
        value: Option<U256>,
    );
    /// trace a single step of VM execution
    fn capture_state(
        &mut self,
        pc: u64,
        op: evm::Opcode,
        gas: u64,
        scope: ScopeContext,
        r_data: &[u8],
        depth: i32,
        err: Option<String>,
    );
    /// is called when EVM enters a new scope (via call, create or selfdestruct)
    fn capture_enter(
        &mut self,
        typ: evm::Opcode,
        from: evm::H160,
        to: evm::H160,
        input: &[u8],
        gas: u64,
        value: Option<U256>,
    );
    /// is called when EVM exits a scope, even if the scope did't execute any code
    fn capture_exit(&mut self, output: &[u8], gas_used: u64, err: Option<String>);
    /// trace an execution fault
    fn capture_fault(
        &mut self,
        pc: u64,
        op: evm::Opcode,
        gas: u64,
        cost: u64,
        scope: Option<ScopeContext>,
        depth: i32,
        err: Option<String>,
    );
    /// is called after the call finished to finalize the tracing
    fn capture_end(
        &mut self,
        output: &[u8],
        gas_used: u64,
        t: std::time::Duration,
        err: Option<String>,
    );
}

pub trait Tracer: EvmLogger {
    /// calls the JavaScript 'result' function and returns its value or any accumulated error
    fn get_result(&mut self) -> Result<serde_json::Value, String>;
}

type Hash = [u8; 32];
type Address = [u8; 20];

const BIGINT: &'static str = include_str!("bigint.js");

fn instruction_name(x: u8) -> Option<&'static str> {
    use crate::types::ec::trace::INSTRUCTIONS;
    use evm::Opcode;

    INSTRUCTIONS
        .get(x as usize)
        .and_then(|i| i.as_ref())
        .map(|i| i.name)
}

#[derive(Debug, Error)]
pub enum Error {}

pub struct StepData {
    pc: u64,
    op: u8,
    gas: u64,
    gas_cost: u64,
    depth: u32,
    memory: Vec<u8>,
    stack: Vec<U256>,
    contract: Contract,
}

#[derive(Value)]
struct VmState {
    pc: u32, // u64
    depth: u32,
    cost: u32,
    gas: u32,
    gas_cost: u32,
}

#[derive(Value)]
#[dukt(
    Peek,
    Push,
    Methods("getPC", "getGas", "getCost", "getDepth", "getRefund", "getError")
)]
struct Log {
    op: OpCode,
    stack: Stack,
    memory: Memory,
    contract: Contract,
    #[hidden]
    vm: VmState,
}

impl Log {
    #[dukt(this = "Log")]
    fn get_pc(&self) -> u32 {
        self.vm.pc
    }

    #[dukt(this = "Log")]
    fn get_gas(&self) -> u32 {
        // todo
        self.vm.gas
    }

    #[dukt(this = "Log")]
    fn get_cost(&self) -> u32 {
        // todo
        self.vm.gas_cost
    }

    #[dukt(this = "Log")]
    fn get_depth(&self) -> u32 {
        // todo
        self.vm.depth
    }

    #[dukt(this = "Log")]
    fn get_refund(&self) -> u32 {
        // todo
        0
    }

    #[dukt(this = "Log")]
    fn get_error(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
struct BigInt(U256);

impl BigInt {
    fn from_str(val: &str) -> Self {
        BigInt(U256::from_dec_str(val).unwrap())
    }

    fn zero() -> Self {
        BigInt(U256::zero())
    }
}

impl PushValue for BigInt {
    fn push_to(self, ctx: &mut dukt::Context) -> u32 {
        if !ctx.get_global_str("bigInt") {
            let res: () = ctx.eval(BIGINT).unwrap();
            ctx.put_global_string("bigInt");
            ctx.get_global_str("bigInt");
        }
        ctx.push_string(&self.0.to_string());
        ctx.call(1).unwrap();
        ctx.stack_top()
    }
}

impl PeekValue for BigInt {
    fn peek_at(ctx: &mut Context, idx: i32) -> Result<Self, dukt::value::PeekError> {
        let idx = if idx < 0 {
            ctx.stack_len() as u32 - (idx.abs() as u32)
        } else {
            idx as u32
        };
        ctx.push_string("toString");
        ctx.call_prop(idx as i32, 0).unwrap();
        String::peek_at(ctx, -1).map(|s| BigInt::from_str(&s))
    }
}

#[derive(Serialize, Deserialize, Value)]
#[dukt(Peek, Push, Serialize, Methods("toNumber", "toString", "isPush"))]
struct OpCode {
    code: u8,
}

impl OpCode {
    #[dukt(this = "OpCode")]
    fn to_number(&self) -> u8 {
        self.code
    }

    #[dukt(this = "OpCode")]
    fn to_string(&self) -> String {
        instruction_name(self.code).unwrap().to_string() // TODO
    }

    #[dukt(this = "OpCode")]
    fn is_push(&self) -> bool {
        // TODO: from table
        (0x60..0x7f).contains(&self.code)
    }
}

#[derive(Value)]
#[dukt(Peek, Push, Methods("slice", "getUint"))]
struct Memory {
    memory: Vec<u8>,
}

impl Memory {
    #[dukt(this = "Memory")]
    fn slice(&self, begin: i32, end: i32) -> &[u8] {
        self.memory
            .get((begin as usize)..(end as usize))
            .unwrap_or(&[])
    }

    #[dukt(this = "Memory")]
    fn get_uint(&self, _offset: u32) -> BigInt {
        BigInt(U256::zero()) // TODO
    }
}

#[derive(Value)]
#[dukt(Peek, Push, Methods("length", "peek"))]
struct Stack {
    stack: Vec<U256>,
}

impl Stack {
    #[dukt(this = "Stack")]
    fn length(&self) -> i32 {
        self.stack.len() as i32
    }

    #[dukt(this = "Stack")]
    fn peek(&self, idx: i32) -> BigInt {
        if idx < 0 || idx >= self.length() {
            return BigInt::zero();
        }
        self.stack.get(idx as usize).map(|u| BigInt(*u)).unwrap()
    }
}

#[derive(Value)]
#[dukt(
    Peek,
    Push,
    Methods("getBalance", "getNonce", "getCode", "getState", "exists")
)]
struct Db {}

impl Db {
    #[dukt(this = "Db")]
    fn get_balance(&self, addr: Address) -> BigInt {
        todo!()
    }

    #[dukt(this = "Db")]
    fn get_nonce(&self, addr: Address) -> i32 {
        todo!()
    }

    #[dukt(this = "Db")]
    fn get_code(&self, addr: Address) -> Vec<u8> {
        todo!()
    }

    #[dukt(this = "Db")]
    fn get_state(&self, key: Hash, addr: Address) -> Vec<u8> {
        todo!()
    }

    #[dukt(this = "Db")]
    fn exists(&self, addr: Address) -> bool {
        todo!()
    }
}

#[derive(Value)]
#[dukt(Peek, Push)]
struct State {
    log: Log,
    frame: Option<Frame>,
    frame_result: Option<FrameResult>,
    db: Db,
}

#[derive(Serialize, Deserialize, Value)]
#[dukt(Peek, Push, Methods("getCaller", "getAddress", "getValue", "getInput"))]
pub struct Contract {
    #[data]
    pub caller: Address,
    #[data]
    pub address: Address,
    #[data]
    pub apparent_value: Option<U256>,
}

impl Contract {
    #[dukt(this = "Contract")]
    fn get_caller(&self) -> Address {
        self.caller.into()
    }

    #[dukt(this = "Contract")]
    fn get_address(&self) -> Address {
        self.address.into()
    }

    #[dukt(this = "Contract")]
    fn get_value(&self) -> BigInt {
        BigInt::zero() // TODO
    }

    #[dukt(this = "Contract")]
    fn get_input(&self) -> Vec<u8> {
        todo!()
    }
}

#[derive(Value)]
#[dukt(Peek, Push, Methods("getType", "getFrom", "getTo", "getGas"))]
struct Frame {
    typ: String,
    from: Address,
    to: Address,
    input: Option<Vec<u8>>,
    gas: u32, // TODO: u64
    value: Option<BigInt>,
}

impl Frame {
    #[dukt(this = "Frame")]
    fn get_type(&self) -> &str {
        &self.typ
    }

    #[dukt(this = "Frame")]
    fn get_from(&self) -> Address {
        self.from
    }

    #[dukt(this = "Frame")]
    fn get_to(&self) -> Address {
        self.to
    }

    #[dukt(this = "Frame")]
    fn get_input(&self) -> &[u8] {
        self.input.as_ref().map(|v| v.as_slice()).unwrap_or(&[])
    }

    #[dukt(this = "Frame")]
    fn get_gas(&self) -> f64 {
        self.gas as f64
    }

    fn get_value(&self) -> BigInt {
        self.value.clone().unwrap_or_else(BigInt::zero)
    }
}

#[derive(Value)]
#[dukt(Peek, Push, Methods("getGasUsed", "getOutput", "getError"))]
struct FrameResult {
    gas_used: u32,
    output: Vec<u8>,
    error_value: Option<String>,
}

impl FrameResult {
    #[dukt(this = "FrameResult")]
    fn get_gas_used(&self) {}
    #[dukt(this = "FrameResult")]
    fn get_output(&self) {}
    #[dukt(this = "FrameResult")]
    fn get_error(&self) {}
}

struct TransactionCtx {
    ty: String,
    from: Address,
    to: Address,
    input: Vec<u8>,
    gas: u64,
    gas_price: u64,
    value: Option<U256>,
    block: u64,
}

pub struct JsTracer {
    ctx: dukt::Context,
    tracer_obj: u32,
    state_obj: u32,
    trace_frames: bool,
    trace_steps: bool,
    state: Option<Rc<RefCell<State>>>,
    transaction: Option<TransactionCtx>,
}

impl EvmLogger for JsTracer {
    fn capture_start(
        &mut self,
        from: evm::H160,
        to: evm::H160,
        create: bool,
        input: &[u8],
        gas: evm::U256,
        value: Option<U256>,
    ) {
        info!("capture start");
        let ty = if create { "CREATE" } else { "CALL" };
        let ctx = TransactionCtx {
            from: from.into(),
            to: to.into(),
            ty: ty.to_string(),
            input: input.to_vec(),
            gas: gas.low_u64(),
            gas_price: 0, // TODO
            value,
            block: 0, // TODO
        };
        self.transaction = Some(ctx);
    }

    fn capture_end(
        &mut self,
        output: &[u8],
        gas_used: u64,
        t: std::time::Duration,
        err: Option<String>,
    ) {
        info!("capture end");
    }

    fn capture_state(
        &mut self,
        pc: u64,
        op: evm::Opcode,
        gas: u64,
        scope: ScopeContext,
        r_data: &[u8],
        depth: i32,
        err: Option<String>,
    ) {
        info!("capture state");

        if let Some(state) = &mut self.state {
            let mut state = state.borrow_mut();

            state.log.vm.cost = 0;
            state.log.vm.depth = depth as u32;
            state.log.vm.gas = gas as u32;
            state.log.vm.gas_cost = 0; // TODO
            state.log.vm.pc = pc as u32;

            state.log.op = OpCode { code: op.0 };
            state.log.stack.stack = scope.stack;
            state.log.memory.memory = scope.memory;
            state.log.contract = scope.contract;
        } else {
            self.init_state(pc, op, gas, scope, r_data, depth, err);
        };

        self.call(true, "step", ["log", "db"]);
    }

    fn capture_enter(
        &mut self,
        typ: evm::Opcode,
        from: evm::H160,
        to: evm::H160,
        input: &[u8],
        gas: u64,
        value: Option<U256>,
    ) {
        info!("capture enter");

        if !self.trace_frames {
            return;
        }

        let frame = Frame {
            typ: instruction_name(typ.0).unwrap().to_string(),
            from: from.into(),
            to: to.into(),
            input: Some(input.to_vec()),
            gas: gas as u32,
            value: value.map(BigInt),
        };

        if let Some(state) = &self.state {
            let mut state = state.borrow_mut();
            state.frame = Some(frame);
        }

        self.call(true, "enter", ["frame"]);
    }

    fn capture_exit(&mut self, output: &[u8], gas_used: u64, err: Option<String>) {
        info!("capture exit");

        if !self.trace_frames {
            return;
        }

        let frame_result = FrameResult {
            gas_used: gas_used as u32,
            output: output.to_vec(),
            error_value: None,
        };

        if let Some(state) = &self.state {
            let mut state = state.borrow_mut();
            state.frame_result = Some(frame_result);
        }

        self.call(true, "exit", ["frameResult"]);
    }

    fn capture_fault(
        &mut self,
        pc: u64,
        op: evm::Opcode,
        gas: u64,
        cost: u64,
        scope: Option<ScopeContext>,
        depth: i32,
        err: Option<String>,
    ) {
        info!("capture fault");
        // TODO
        self.call(true, "fault", ["log", "db"]);
    }
}

impl Tracer for JsTracer {
    fn get_result(&mut self) -> Result<serde_json::Value, String> {
        // TODO: ctx
        match self.call(false, "result", ["ctx", "db"]) {
            Some(s) => Ok(serde_json::from_str(&s).unwrap()),
            None => Err("no trace".to_string()),
        }
    }
}

impl JsTracer {
    pub fn new(code: &str) -> Result<Self, Error> {
        let ctx = dukt::Context::default();

        let mut tracer = JsTracer {
            ctx,
            tracer_obj: 0,
            state_obj: 0,
            trace_frames: false,
            trace_steps: false,
            state: None,
            transaction: None,
        };
        tracer.init_global_objects();
        tracer.init_global_functions();
        tracer.init_code(code);

        Ok(tracer)
    }

    fn init_code(&mut self, code: &str) {
        println!("{}", code);
        let res = self.ctx.eval::<()>(&format!("({})", code));
        self.tracer_obj = self.ctx.stack_top();
        println!("pushed tracer @ {} {:?}", self.tracer_obj, res);
        let has_step = self.ctx.get_prop(-1, "step");
        self.ctx.pop();
        let has_result = self.ctx.get_prop(-1, "result");
        self.ctx.pop();
        let has_fault = self.ctx.get_prop(-1, "fault");
        self.ctx.pop();
        let has_enter = self.ctx.get_prop(-1, "enter");
        self.ctx.pop();
        let has_exit = self.ctx.get_prop(-1, "exit");
        self.ctx.pop();

        if has_enter != has_exit {
            panic!("must have enter & exit or none");
        }
        self.trace_frames = has_enter && has_exit;
        self.trace_steps = has_step;

        println!(
            "step {} result {} fault {} enter {} exit {}",
            has_step, has_result, has_fault, has_enter, has_exit
        );
    }

    fn init_global_objects(&mut self) {
        self.state_obj = self.ctx.push_object();
    }

    fn init_global_functions(&mut self) {
        #[dukt]
        fn to_hex(_: &mut dukt::Context, bytes: Vec<u8>) -> String {
            hex::encode(bytes)
        }
        self.ctx.register_function("toHex", ToHex);

        #[dukt]
        fn to_word(ctx: &mut dukt::Context) -> Hash {
            let mut hash = [0; 32];
            if let Some(data) = ctx.get_buffer_opt(-1) {
                hash.copy_from_slice(&data[0..32]);
            } else {
                let s = ctx.get_string(-1);
                hex::decode_to_slice(s, &mut hash);
            }
            ctx.pop();
            hash
        }
        self.ctx.register_function("toWord", ToWord);

        #[dukt]
        fn to_address(ctx: &mut dukt::Context) -> Address {
            let mut addr = [0; 20];
            if let Some(data) = ctx.get_buffer_opt(-1) {
                addr.copy_from_slice(&data[0..20]);
            } else {
                let s = ctx.get_string(-1);
                hex::decode_to_slice(s, &mut addr);
            }
            ctx.pop();
            addr
        }
        self.ctx.register_function("toAddress", ToAddress);

        #[dukt]
        fn to_contract(ctx: &mut dukt::Context) -> Address {
            let mut from_addr = [0; 20];
            if let Some(data) = ctx.get_buffer_opt(-2) {
                from_addr.copy_from_slice(&data[0..20]);
            } else {
                let s = ctx.get_string(-2);
                hex::decode_to_slice(s, &mut from_addr);
            };
            let nonce: u32 = ctx.get_uint(-1);
            ctx.pop_n(2);
            todo!("rlp.encode this")
        }
        self.ctx.register_function("toContract", ToContract);

        #[dukt]
        fn is_precompiled(ctx: &mut dukt::Context) -> bool {
            // TODO: wtf is this
            false
        }
        self.ctx.register_function("isPrecompiled", IsPrecompiled);

        #[dukt]
        fn slice(ctx: &mut dukt::Context, start: i32, end: i32) -> Vec<u8> {
            let buf = ctx.get_buffer(-1);
            if start < 0 || start > end || end as usize > buf.len() {
                return Vec::new();
            }
            buf.get((start as usize)..(end as usize))
                .map(|slice| slice.to_vec())
                .unwrap_or_else(Vec::new)
        }
        self.ctx.register_function("slice", Slice);
    }

    fn init_state(
        &mut self,
        pc: u64,
        op: evm::Opcode,
        gas: u64,
        scope: ScopeContext,
        r_data: &[u8],
        depth: i32,
        err: Option<String>,
    ) {
        let vm_state = VmState {
            cost: 0,             // TODO
            depth: depth as u32, // TODO
            gas: gas as u32,     // TODO
            pc: pc as u32,
            gas_cost: 0, // TODO
        };
        let log = Log {
            op: OpCode { code: op.0 },
            stack: Stack { stack: scope.stack },
            memory: Memory {
                memory: scope.memory,
            },
            contract: scope.contract,
            vm: vm_state,
        };
        let tx = self.transaction.as_ref().unwrap();
        let state = State {
            log,
            frame: None,
            frame_result: None,
            db: Db {},
        };
        let stack_top = self.ctx.stack_top();
        let idx = state.push_to(&mut self.ctx); // TODO
        let stack_top = self.ctx.stack_top();
        self.ctx.swap(self.state_obj as i32, idx as i32);
        self.ctx.pop();
    }

    fn call(
        &mut self,
        no_ret: bool,
        method: &str,
        args: impl IntoIterator<Item = &'static str>,
    ) -> Option<String> {
        self.ctx.push_string(method);
        let mut n_args = 0;
        for arg in args {
            self.ctx.get_prop(self.state_obj as i32, arg);
            n_args += 1;
        }
        self.ctx.call_prop(self.tracer_obj as i32, n_args);

        if no_ret {
            self.ctx.pop();
            return None;
        }

        self.ctx.eval::<()>("(JSON.stringify)").unwrap();
        self.ctx.swap(-1, -2);
        self.ctx.call(1).unwrap();
        return Some(String::peek_at(&mut self.ctx, -1).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_int_test() {
        let mut ctx = Context::default();
        let a = BigInt::from_str("5");
        let idx = a.push_to(&mut ctx);

        ctx.push_string("toString");
        ctx.call_prop(idx as i32, 0).unwrap();

        let s = String::peek_at(&mut ctx, -1).unwrap();
        assert_eq!(s, "5");

        let aa = BigInt::peek_at(&mut ctx, idx as i32).unwrap();
        println!("{:?}", aa);
        assert_eq!(BigInt::from_str("5"), aa);
    }

    #[test]
    fn memory_test() {
        let mut ctx = Context::default();
        let mem = Memory {
            memory: vec![1u8, 2, 3, 4],
        };
        //let top_stack = ctx.stack_top();
        let idx = mem.push_to(&mut ctx);
        println!("mem idx = {}", idx);
        let top_stack = ctx.stack_top();
        assert_eq!(top_stack, idx);
        let m = Memory::peek_at(&mut ctx, idx as i32).unwrap();
        assert_eq!(&m.memory, &[1, 2, 3, 4]);
        ctx.push_string("slice");
        ctx.push_int(0);
        ctx.push_int(2);
        ctx.call_prop(idx as i32, 2).unwrap();
        let x: Vec<u8> = ctx.peek(-1).unwrap();
        assert_eq!(x, &[1, 2]);
    }

    #[test]
    fn test_stack() {
        let mut ctx = Context::default();
        let five = U256::from_dec_str("5").unwrap();
        let stack = Stack {
            stack: vec![U256::zero(), five],
        };
        let idx = stack.push_to(&mut ctx);
        ctx.push_string("length");
        ctx.call_prop(idx as i32, 0).unwrap();
        assert_eq!(2, ctx.pop_value::<u32>().unwrap());

        ctx.push_string("peek");
        ctx.push_int(1);
        ctx.call_prop(idx as i32, 1).unwrap();
        assert_eq!(BigInt::from_str("5"), ctx.pop_value::<BigInt>().unwrap());
    }

    #[test]
    fn tracer_test() {
        const TRACER: &'static str = r#"{data: [], fault: function(log) {}, step: function(log) { if(log.op.toString() == "CALL") this.data.push(log.stack.peek(0)); }, result: function() { return this.data; }}"#;

        let dump_opcode_tracer = r#"{data: [], fault: function(log) {}, step: function(log) { this.data.push(log.getPC() + ":" + log.op.toString()) }, result: function() { return this.data; }}"#;

        let mut tracer = JsTracer::new(dump_opcode_tracer).unwrap();
        tracer.capture_start(
            H160::from_slice(&[0; 20]),
            H160::from_slice(&[1; 20]),
            false,
            &[],
            evm::U256::zero(),
            None,
        );

        tracer.capture_state(
            0,
            evm::Opcode::ADD,
            1,
            ScopeContext {
                stack: vec![],
                memory: vec![],
                contract: Contract {
                    caller: [0; 20],
                    address: [1; 20],
                    apparent_value: None,
                },
            },
            &[],
            1,
            None,
        );

        tracer.capture_state(
            1,
            evm::Opcode::MUL,
            1,
            ScopeContext {
                stack: vec![],
                memory: vec![],
                contract: Contract {
                    caller: [0; 20],
                    address: [1; 20],
                    apparent_value: None,
                },
            },
            &[],
            1,
            None,
        );

        let res = tracer.get_result().unwrap();
        let s = serde_json::to_string(&res).unwrap();
        println!("{}", s);
        assert_eq!(s, r#"["0:ADD","1:MUL"]"#)
    }
}
