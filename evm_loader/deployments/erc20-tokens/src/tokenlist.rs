use std::fmt;
use serde::{ Deserialize };

#[derive(Clone)]
#[derive(Deserialize)]
pub struct Erc20Specs {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Clone)]
#[derive(Deserialize)]
pub struct Erc20Addrs {
    pub solana_mainnet_mint_pubkey: String,
    pub solana_devnets_mint_pubkey: String,
    pub neonevm_erc20token_address: Option<String>,
}

#[derive(Clone)]
#[derive(Deserialize)]
pub struct Erc20Item {
    pub specs: Erc20Specs,
    pub addrs: Erc20Addrs,
}

impl fmt::Display for Erc20Specs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [ {} ]", self.name, self.symbol)
    }
}


pub fn read_erc20_items(filepath: &str) -> Vec<Erc20Item> {

    let file = std::fs::File::open(filepath).unwrap();
    let reader = std::io::BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}
