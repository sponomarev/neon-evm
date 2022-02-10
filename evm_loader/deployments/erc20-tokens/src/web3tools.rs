use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use serde_json::{to_string, to_vec};
use serde_json::Value;

use web3::types::Address;
use web3::Transport;
use web3::signing::Key;
use web3::contract::{
    Contract,
    tokens::Tokenize,
};

pub async fn deploy_contract<T,K,P>(web3: &web3::Web3<T>, key: K, abi_file: &str, params: P, opt_linker: Option<HashMap<&str,Address>>) ->  Result<Contract<T>, web3::contract::deploy::Error>
where
    T: Transport,
    K: Key,
    P: Tokenize,
{
    // open the abi file
    let abi = File::open(abi_file);
    if abi.is_err() {
        println!("Failed to open {}\n", abi_file);
    }

    // read the abi file
    let mut abi_data = String::new();
    let bytes_read = abi.unwrap().read_to_string(&mut abi_data);
    if bytes_read.is_err() {
        println!("Failed to read from {}\n", abi_file);
    }

    let lib: Value = serde_json::from_str(&abi_data).unwrap();
    let lib_abi: Vec<u8> = to_vec(&lib["abi"]).unwrap();
    let lib_code =
        if lib["bytecode"] == Value::Null {
            to_string(&lib["evm"]["bytecode"]["object"]).unwrap()
        } else {
            to_string(&lib["bytecode"]).unwrap()
        };

    let builder = 
        if let Some(linker) = opt_linker {
            Contract::deploy_from_truffle(web3.eth(), &lib_abi, linker).unwrap()
        } else {
            Contract::deploy(web3.eth(), &lib_abi).unwrap()
        };
    
    builder
        .confirmations(0)
        .sign_with_key_and_execute(lib_code, params, key, None)
        .await
}

pub fn _get_contract_from_abi_file(web3: &web3::Web3<web3::transports::Http>, abi_file_path: &str, contract_address: Address) -> Result<Contract<web3::transports::Http>,()> {

    // open the abi file
    let abi = File::open(abi_file_path);
    if abi.is_err() {
        println!("Failed to open {}\n", abi_file_path);
        return Err(());
    }

    // read the abi file
    let mut abi_data = String::new();
    let bytes_read = abi.unwrap().read_to_string(&mut abi_data);
    if bytes_read.is_err() {
        println!("Failed to read from {}\n", abi_file_path);
        return Err(());
    }

    let lib: Value = serde_json::from_str(&abi_data).unwrap();
    let lib_abi: Vec<u8> = to_vec(&lib["abi"]).unwrap();

    Contract::from_json(web3.eth(), contract_address, &lib_abi).map_err(|_|())
}

pub trait AsEip55 {
    fn as_eip55(&self) -> String;
}

impl AsEip55 for Address {
    fn as_eip55(&self) -> String {
        eip55::checksum(&format!("{:?}",self))
    }
}

pub fn array_u8_32_from_str(s: &str) -> [u8; 32] {
    let bytes: Vec<u8> = bs58::decode(s).into_vec().unwrap();
    let mut a: [u8; 32] = [0; 32];
    for (i,value) in bytes.into_iter().enumerate() {
        a[i] = value;
    };
    a
}
