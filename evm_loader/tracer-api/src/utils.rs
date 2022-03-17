use solana_account_decoder::parse_token::{token_amount_to_ui_amount, UiTokenAmount};
use solana_program::program_pack::Pack;
use solana_sdk::account::{Account, ReadableAccount};
use spl_token::{native_mint, state::Account as TokenAccount};

use evm_loader::config::token_mint as neon_mint;

pub fn parse_token_amount(account: &Account) -> Option<UiTokenAmount> {
    (account.owner() == &spl_token::id()).then(|| ())?;

    let token_account = TokenAccount::unpack(account.data()).ok()?;
    let mint = token_account.mint;

    let decimals = match mint {
        mint if mint == neon_mint::ID => neon_mint::DECIMALS,
        mint if mint == native_mint::ID => native_mint::DECIMALS,
        // TODO: rest, consider having a static map to hold known mints
        _ => return None,
    };

    Some(token_amount_to_ui_amount(token_account.amount, decimals))
}
