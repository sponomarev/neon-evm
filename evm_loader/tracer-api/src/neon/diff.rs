use std::collections::{BTreeMap, HashMap};
use std::iter;

use tracing::warn;

use evm::{backend::Apply, Transfer, H160, U256, H256};

use super::{account_storage::EmulatorAccountStorage, provider::Provider, To};
use crate::types::ec::account_diff::AccountDiff;
use crate::types::ec::pod_account::{diff_pod, PodAccount};
use crate::types::ec::state_diff::StateDiff;
use evm_loader::account_storage::AccountStorage;

#[derive(Debug, Clone, Copy)]
enum Sign {
    Pos,
    Neg,
}

impl Sign {
    fn swap(&mut self) {
        match self {
            Sign::Pos => *self = Sign::Neg,
            Sign::Neg => *self = Sign::Pos,
        }
    }
}

pub fn prepare_state_diff<P, A, I, T>(
    accounts: &EmulatorAccountStorage<P>,
    applies: A,
    transfers: T,
) -> StateDiff
where
    P: Provider,
    A: IntoIterator<Item = Apply<I>>,
    I: IntoIterator<Item = (U256, U256)>,
    T: IntoIterator<Item = Transfer>,
{
    let mut state_diff = StateDiff {
        raw: BTreeMap::new(),
    };

    let mut balance_diff = collect_balance_changes(transfers);

    for apply in applies {
        let diff = match apply {
            Apply::Modify {
                address,
                nonce,
                code_and_valids,
                storage,
                reset_storage: _,
            } => {
                let balance = balance_diff.remove(&address);
                modify(accounts, address, nonce, code_and_valids, balance, storage)
                    .map(|diff| (address, diff))
            }
            Apply::Delete { address } => {
                let _balance = balance_diff.remove(&address);
                let old_account = get_account(accounts, address, iter::empty());
                diff_pod(old_account.as_ref(), None).map(|diff| (address, diff))
            }
        };
        if let Some((address, diff)) = diff {
            state_diff.raw.insert(address.to(), diff);
        }
    }

    if !balance_diff.is_empty() {
        warn!("applies did not completely drain transfer map");
    }

    for (address, balance) in balance_diff.into_iter() {
        if let Some(old) = get_account(accounts, address, iter::empty()) {
            let diff = modify(
                accounts,
                address,
                old.nonce,
                old.code.map(|code| (code, Vec::new())),
                Some(balance),
                iter::empty(),
            );
            if let Some(diff) = diff {
                state_diff.raw.insert(address.to(), diff);
            }
        } else {
            warn!("could not apply balance update to {}", address);
        }
    }

    state_diff
}

fn collect_balance_changes<I>(transfers: I) -> HashMap<H160, (Sign, U256)>
where
    I: IntoIterator<Item = Transfer>,
{
    fn apply_balance_update(current: &mut (Sign, U256), sign: Sign, value: U256) {
        match (current.0, sign) {
            (Sign::Neg, Sign::Neg) | (Sign::Pos, Sign::Pos) => current.1 += value,
            (Sign::Neg, Sign::Pos) | (Sign::Pos, Sign::Neg) if current.1 < value => {
                current.1 = value - current.1;
                current.0.swap();
            }
            (Sign::Neg, Sign::Pos) | (Sign::Pos, Sign::Neg) => current.1 -= value,
        }
    }

    let mut balance_diff = HashMap::<_, (Sign, U256)>::new();
    transfers.into_iter().for_each(|transfer| {
        let source = balance_diff
            .entry(transfer.source)
            .or_insert((Sign::Pos, U256::zero()));
        apply_balance_update(source, Sign::Neg, transfer.value);

        let target = balance_diff
            .entry(transfer.target)
            .or_insert((Sign::Pos, U256::zero()));
        apply_balance_update(target, Sign::Pos, transfer.value);
    });

    balance_diff
}

fn get_account<I, P>(
    accounts: &EmulatorAccountStorage<P>,
    address: H160,
    keys: I,
) -> Option<PodAccount>
where
    I: IntoIterator<Item = U256>,
    P: Provider,
{
    let balance = accounts.balance(&address);
    let nonce = accounts.nonce(&address);
    let code = accounts.code(&address);
    let storage = keys
        .into_iter()
        .map(|key| (H256::from(key), H256::from(accounts.storage(&address, &key))))
        .collect();

    let pod = PodAccount {
        balance: balance,
        nonce: nonce,
        code: Some(code),
        storage: storage,
    };
    Some(pod)
}

fn modify<P, I>(
    accounts: &EmulatorAccountStorage<P>,
    address: H160,
    nonce: U256,
    code_and_valids: Option<(Vec<u8>, Vec<u8>)>,
    balance: Option<(Sign, U256)>,
    storage: I,
) -> Option<AccountDiff>
where
    P: Provider,
    I: IntoIterator<Item = (U256, U256)>,
{
    let storage: BTreeMap<_, _> = storage.into_iter().collect();
    let mut old_pod = get_account(accounts, address, storage.keys().copied());
    let old_balance = old_pod
        .as_ref()
        .map_or(U256::zero(), |pod| pod.balance);
    let balance = balance.map_or(old_balance, |(sign, value)| match sign {
        Sign::Pos => old_balance + value,
        Sign::Neg => old_balance - value,
    });

    let mut new_pod = PodAccount {
        balance: balance,
        nonce: nonce,
        storage: storage.into_iter().map(|(k, v)| (H256::from(k), H256::from(v))).collect(),
        code: code_and_valids.map(|(code, _valids)| code),
    };

    diff_pod(old_pod.as_ref(), Some(&new_pod))
}
