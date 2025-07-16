//! Auth Mint Alkane Contract
//!
//! A modernized and secure version of the free mint alkane contract that follows
//! current best practices and security patterns while providing full functionality
//! of a standard token plus free mint capabilities.

use alkanes_runtime::storage::StoragePointer;
use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder};
use alkanes_support::response::CallResponse;
use alkanes_support::utils::overflow_error;
use alkanes_support::{context::Context, parcel::AlkaneTransfer};
use anyhow::{anyhow, Result};
use bitcoin::hashes::Hash;
use bitcoin::Txid;
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;
use std::sync::Arc;

/// Constants for token identification
pub const ALKANE_FACTORY_OWNED_TOKEN_ID: u128 = 0x0fff;
pub const ALKANE_FACTORY_FREE_MINT_ID: u128 = 0x0ffe;

/// Returns a StoragePointer for the token name
fn name_pointer() -> StoragePointer {
    StoragePointer::from_keyword("/name")
}

/// Returns a StoragePointer for the token symbol
fn symbol_pointer() -> StoragePointer {
    StoragePointer::from_keyword("/symbol")
}

/// Trims a u128 value to a String by removing trailing zeros
pub fn trim(v: u128) -> String {
    String::from_utf8(
        v.to_le_bytes()
            .into_iter()
            .fold(Vec::<u8>::new(), |mut r, v| {
                if v != 0 {
                    r.push(v)
                }
                r
            }),
    )
    .unwrap()
}

/// TokenName struct to hold two u128 values for the name
#[derive(Default, Clone, Copy)]
pub struct TokenName {
    pub part1: u128,
    pub part2: u128,
}

impl From<TokenName> for String {
    fn from(name: TokenName) -> Self {
        // Trim both parts and concatenate them
        format!("{}{}", trim(name.part1), trim(name.part2))
    }
}

impl TokenName {
    pub fn new(part1: u128, part2: u128) -> Self {
        Self { part1, part2 }
    }
}

pub struct ContextHandle(());

#[cfg(test)]
impl ContextHandle {
    /// Get the current transaction bytes
    pub fn transaction(&self) -> Vec<u8> {
        // This is a placeholder implementation that would normally
        // access the transaction from the runtime context
        Vec::new()
    }
}

/// MintableToken trait provides common token functionality
pub trait MintableToken: AlkaneResponder {
    /// Get the token name
    fn name(&self) -> String {
        String::from_utf8(self.name_pointer().get().as_ref().clone())
            .expect("name not saved as utf-8, did this deployment revert?")
    }

    /// Get the token symbol
    fn symbol(&self) -> String {
        String::from_utf8(self.symbol_pointer().get().as_ref().clone())
            .expect("symbol not saved as utf-8, did this deployment revert?")
    }

    /// Set the token name and symbol
    fn set_name_and_symbol(&self, name: TokenName, symbol: u128) {
        let name_string: String = name.into();
        self.name_pointer()
            .set(Arc::new(name_string.as_bytes().to_vec()));
        self.set_string_field(self.symbol_pointer(), symbol);
    }

    /// Get the pointer to the token name
    fn name_pointer(&self) -> StoragePointer {
        name_pointer()
    }

    /// Get the pointer to the token symbol
    fn symbol_pointer(&self) -> StoragePointer {
        symbol_pointer()
    }

    /// Set a string field in storage
    fn set_string_field(&self, mut pointer: StoragePointer, v: u128) {
        pointer.set(Arc::new(trim(v).as_bytes().to_vec()));
    }

    /// Get the pointer to the total supply
    fn total_supply_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/totalsupply")
    }

    /// Get the total supply
    fn total_supply(&self) -> u128 {
        self.total_supply_pointer().get_value::<u128>()
    }

    /// Set the total supply
    fn set_total_supply(&self, v: u128) {
        self.total_supply_pointer().set_value::<u128>(v);
    }

    /// Increase the total supply
    fn increase_total_supply(&self, v: u128) -> Result<()> {
        self.set_total_supply(
            overflow_error(self.total_supply().checked_add(v))
                .map_err(|_| anyhow!("total supply overflow"))?,
        );
        Ok(())
    }

    /// Mint new tokens
    fn mint(&self, context: &Context, value: u128) -> Result<AlkaneTransfer> {
        self.increase_total_supply(value)?;
        Ok(AlkaneTransfer {
            id: context.myself.clone(),
            value,
        })
    }
}

/// MintableAlkane implements a free mint token contract with security features
#[derive(Default)]
pub struct MintableAlkane(());

impl MintableToken for MintableAlkane {}

/// Message enum for opcode-based dispatch
#[derive(MessageDispatch)]
enum MintableAlkaneMessage {
    /// Initialize the token with configuration
    #[opcode(0)]
    Initialize {
        /// Maximum supply cap (0 for unlimited)
        cap: u128,
        /// Token name part 1
        name_part1: u128,
        /// Token name part 2
        name_part2: u128,
        /// Token symbol
        symbol: u128,
    },

    /// Get the token name
    #[opcode(99)]
    #[returns(String)]
    GetName,

    /// Get the token symbol
    #[opcode(100)]
    #[returns(String)]
    GetSymbol,

    /// Get the total supply
    #[opcode(101)]
    #[returns(u128)]
    GetTotalSupply,

    /// Get the maximum supply cap
    #[opcode(102)]
    #[returns(u128)]
    GetCap,
}

impl MintableAlkane {
    /// Get the pointer to the minted counter
    pub fn minted_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/minted")
    }

    /// Get the total minted count
    pub fn minted(&self) -> u128 {
        self.minted_pointer().get_value::<u128>()
    }

    /// Set the total minted count
    pub fn set_minted(&self, v: u128) {
        self.minted_pointer().set_value::<u128>(v);
    }

    /// Increment the mint counter
    pub fn increment_mint(&self) -> Result<()> {
        self.set_minted(
            overflow_error(self.minted().checked_add(1u128))
                .map_err(|_| anyhow!("mint counter overflow"))?,
        );
        Ok(())
    }

    /// Get the pointer to the supply cap
    pub fn cap_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/cap")
    }

    /// Get the supply cap
    pub fn cap(&self) -> u128 {
        self.cap_pointer().get_value::<u128>()
    }

    /// Set the supply cap (0 means unlimited)
    pub fn set_cap(&self, v: u128) {
        self.cap_pointer()
            .set_value::<u128>(if v == 0 { u128::MAX } else { v });
    }

    /// Check if a transaction hash has been used for minting
    pub fn has_tx_hash(&self, txid: &Txid) -> bool {
        StoragePointer::from_keyword("/tx-hashes/")
            .select(&txid.as_byte_array().to_vec())
            .get_value::<u8>()
            == 1
    }

    /// Add a transaction hash to the used set
    pub fn add_tx_hash(&self, txid: &Txid) -> Result<()> {
        StoragePointer::from_keyword("/tx-hashes/")
            .select(&txid.as_byte_array().to_vec())
            .set_value::<u8>(0x01);
        Ok(())
    }

    /// Initialize the token with configuration
    fn initialize(
        &self,
        cap: u128,
        name_part1: u128,
        name_part2: u128,
        symbol: u128,
    ) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Prevent multiple initializations
        self.observe_initialization()
            .map_err(|_| anyhow!("Contract already initialized"))?;

        // Set configuration
        self.set_cap(cap);
        // self.set_data()?;

        let name = TokenName::new(name_part1, name_part2);
        <Self as MintableToken>::set_name_and_symbol(self, name, symbol);

        response.alkanes.0.push(self.mint(&context, cap)?);
        Ok(response)
    }

    /// Get the token name
    fn get_name(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.name().into_bytes().to_vec();
        Ok(response)
    }

    /// Get the token symbol
    fn get_symbol(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.symbol().into_bytes().to_vec();
        Ok(response)
    }

    /// Get the total supply
    fn get_total_supply(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.total_supply().to_le_bytes().to_vec();
        Ok(response)
    }

    /// Get the maximum supply cap
    fn get_cap(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.cap().to_le_bytes().to_vec();
        Ok(response)
    }

}

impl AlkaneResponder for MintableAlkane {}

// Use the MessageDispatch macro for opcode handling
declare_alkane! {
    impl AlkaneResponder for MintableAlkane {
        type Message = MintableAlkaneMessage;
    }
}
