use std::str::FromStr;

use secp256k1::{ SecretKey };

use web3::types::{ Address, U256 };
use web3::signing::{ Key, SecretKeyRef };
use web3::contract::{ Contract };

use clap::{ Arg, App };

mod network;
mod tokenlist;
mod etherstools;
mod web3tools;

use network::Network;
use tokenlist::{ Erc20Item, read_erc20_items };
use etherstools::{ EthersUtils };
use web3tools::{ AsEip55, deploy_contract, array_u8_32_from_str };


#[tokio::main(flavor = "current_thread")]
async fn main() {

    let matches =
        App::new("Deploy ERC20 Tokens Program")
            .version("0.3")
            .about("Deploy ERC20-Wrapper tokens")
            .arg(Arg::new("network")
                .short('t')
                .long("to")
                .value_name("NETWORK")
                .required(true)
                .takes_value(true)
            )
            .arg(Arg::new("abi")
                .short('a')
                .long("abi")
                .value_name("ERC20WRAPPER ABI")
                .required(true)
            )
            .arg(Arg::new("key")
                .short('k')
                .long("key")
                .value_name("KEY")
                .required(true)
            )
            .arg(Arg::new("tokenlist")
                .short('l')
                .long("tokenlist")
                .value_name("TOKENS")
                .required(true)
            )
            .get_matches();

    let network: Network = 
            matches.value_of("network")
                .map(|alias| Network::from_alias(alias) )
                .unwrap();
    let abi_path: &str = 
            matches.value_of("abi")
                .unwrap();
    let eth_private_key: &str =
            matches.value_of("key")
                .unwrap();
    let key: SecretKey =
            SecretKey::from_str(&eth_private_key)
                .unwrap();
    let ethers_utils: EthersUtils =
            EthersUtils::new(&eth_private_key);
    let token_infos: Vec<Erc20Item> =
            matches.value_of("tokenlist")
                .map(|path| read_erc20_items(path) )
                .unwrap();

    let transport = web3::transports::Http::new(network.get_proxy_url()).unwrap();
    let web3 = web3::Web3::new(transport);
    
    println!("\n----- Deployment of ERC20 Bridge Tokens -----\n");

    let chain_id = web3.eth().chain_id().await.unwrap();
    println!("chain_id :  {}", chain_id);
    assert_eq!(chain_id, U256::from(network.get_chain_id()));
    
    let address: Address = SecretKeyRef::new(&key).address();
    println!("Deployer Address: {}", address.as_eip55());

    let balance = web3.eth().balance(address, None).await.unwrap();
    println!("Balance of {}: {}", address.as_eip55(), balance);
    assert!(balance > U256::zero());

    let nonce = web3.eth().transaction_count(address, None).await.unwrap();
    println!("Current Nonce of {}: {}", address.as_eip55(), nonce);
    let transaction_count = nonce.as_usize();

    println!("");

    for (counter, token_info) in token_infos.iter().enumerate() {

        let presumed_erc20_address: Address = ethers_utils.get_contract_address(counter.into());
        let neonevm_erc20token_address: Address =
            if let Some(neonevm_erc20token_address_str) = &token_info.addrs.neonevm_erc20token_address {
                let neonevm_erc20token_address: Address =Address::from_str(&neonevm_erc20token_address_str).unwrap();
                assert_eq!(presumed_erc20_address, neonevm_erc20token_address);
                neonevm_erc20token_address
            } else {
                presumed_erc20_address
            };
        
        if transaction_count <= counter {

            let address_spl: &str =
                if network == Network::Mainnet {
                    &token_info.addrs.solana_mainnet_mint_pubkey
                } else {
                    &token_info.addrs.solana_devnets_mint_pubkey
                };
            
            let contract_params: (String, String, [u8; 32]) = (token_info.specs.name.clone(), token_info.specs.symbol.clone(), array_u8_32_from_str(address_spl));
            
            let erc20_contract: Contract<web3::transports::Http> = 
                deploy_contract(&web3, &key, abi_path, contract_params, None)
                    .await
                    .unwrap();
            
            println!("Deployed {} -> {}", token_info.specs, erc20_contract.address().as_eip55());
            assert_eq!(presumed_erc20_address, erc20_contract.address());
        } else {
            println!("Exists {} at {}", token_info.specs, neonevm_erc20token_address.as_eip55());
        };
    };
    println!("");
}
