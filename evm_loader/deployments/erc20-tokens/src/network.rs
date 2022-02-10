#[derive(Eq, PartialEq)]
pub enum Network {
    Mainnet,
    Testnet,
    Devnet,
    ReleaseStand,
    NightStand,
    Local,
}

impl Network {
    pub fn from_alias(alias: &str) -> Network {
        match alias {
            "mainnet"       => Self::Mainnet,
            "testnet"       => Self::Testnet,
            "devnet"        => Self::Devnet,
            "release"       => Self::ReleaseStand,
            "nightly"       => Self::NightStand,
            "local"         => Self::Local,
            _               => panic!("Wrong network!"),
        }
    }
    pub fn get_chain_id(&self) -> u64 {
        match self {
            Network::Mainnet        => { 245022934 },
            Network::Testnet        => { 245022940 },
            Network::Devnet         => { 245022926 },
            Network::ReleaseStand   |
            Network::NightStand     |
            Network::Local          => { 111 },
        }
    }
    pub fn _get_str_ident(&self) -> &str {
        match self {
            Network::Mainnet        => { unimplemented!() },
            Network::Testnet        => { "testnet" },
            Network::Devnet         => { "devnet" },
            Network::ReleaseStand   => { "teststand1" },
            Network::NightStand     => { "nightstand" },
            Network::Local          => { "local" },
        }
    }
    pub fn _get_solana_url(&self) -> &str {
        match self {
            Network::Mainnet        => { "https://api.mainnet-beta.solana.com" },
            Network::Testnet        => { "https://api.testnet.solana.com" },
            Network::Devnet         => { "https://api.devnet.solana.com" },
            Network::ReleaseStand   => { "https://proxy.teststand.neontest.xyz/node-solana" },
            Network::NightStand     => { "https://proxy.teststand2.neontest.xyz/node-solana" },
            Network::Local          => { "http://localhost:8899" },
        }
    }
    pub fn get_proxy_url(&self) -> &str {
        match self {
            Network::Mainnet        => { unimplemented!() },
            Network::Testnet        => { "https://proxy.testnet.neonlabs.org/solana" },
            Network::Devnet         => { "https://proxy.devnet.neonlabs.org/solana" },
            Network::ReleaseStand   => { "https://proxy.teststand.neontest.xyz/solana" },
            Network::NightStand     => { "https://proxy.teststand2.neontest.xyz/solana" },
            Network::Local          => { "http://localhost:9090/solana" },
        }
    }
    pub fn _get_evm_loader_program_id(&self) -> &str {
        match self {
            Network::Mainnet        => { unimplemented!() },
            Network::Testnet        |
            Network::Devnet         => { "eeLSJgWzzxrqKv1UxtRVVH8FX3qCQWUs9QuAjJpETGU" },
            Network::ReleaseStand   |
            Network::NightStand     |
            Network::Local          => { "53DfF883gyixYNXnM7s5xhdeyV8mVk9T4i2hGV9vG9io" },
        }
    }
    pub fn _get_neon_mint_id(&self) -> &str {
        match self {
            Network::Mainnet        => { unimplemented!() },
            Network::Testnet        |
            Network::Devnet         => { "89dre8rZjLNft7HoupGiyxu3MNftR577ZYu8bHe2kK7g" },
            Network::ReleaseStand   |
            Network::NightStand     |
            Network::Local          => { "HPsV9Deocecw3GeZv1FkAPNCBRfuVyfw9MMwjwRe1xaU" },
        }
    }
}
