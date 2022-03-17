// ec module was copied from OpenEthereum and is kept as close
// as possible to original
#[rustfmt::skip]
#[allow(clippy::all)]
pub mod ec;

use evm::{H160, H256};
use parity_bytes::ToPretty;

type Bytes = Vec<u8>;
#[derive(Debug)]
pub struct TxMeta<T> {
    pub slot: u64,
    pub from: H160,
    pub to: Option<H160>,
    pub eth_signature: H256,
    pub value: T,
}

impl<T> TxMeta<T> {
    pub fn split(self) -> (TxMeta<()>, T) {
        let new_meta = TxMeta {
            slot: self.slot,
            from: self.from,
            to: self.to,
            eth_signature: self.eth_signature,
            value: (),
        };

        (new_meta, self.value)
    }

    pub fn wrap<U>(self, new_value: U) -> TxMeta<U> {
        TxMeta {
            slot: self.slot,
            from: self.from,
            to: self.to,
            eth_signature: self.eth_signature,
            value: new_value,
        }
    }
}
