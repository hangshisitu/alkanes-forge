use alkanes_runtime::{
    declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, storage::StoragePointer,
    token::Token,
};
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;
use std::result::Result::Ok;

use alkanes_support::{
    cellpack::Cellpack,
    id::AlkaneId,
    parcel::{AlkaneTransfer, AlkaneTransferParcel},
    response::CallResponse,
};

use anyhow::Result;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use types_support::staking::Staking;

// Contract configuration constants
const CONTRACT_NAME: &str = "Forge Stake Pool";
const CONTRACT_SYMBOL: &str = "FSP";
const TOKEN_CAP: u128 = 100000000000000000;
const MINING_ONE_BLOCK_VOLUME: u64 = 1003086419753;
const MINING_FIRST_HEIGHT: u64 = 450; // Mining start block height
const MINING_LAST_HEIGHT: u64 = MINING_FIRST_HEIGHT + 144 * 360 - 1; // Mining end block height
const MIN_STAKE_VALUE: u64 = 1000;
const PROFIT_RELEASE_HEIGHT: u64 = 144 * 180;

// Token deployment constants
const COIN_TEMPLATE_ID: u128 = 7; // forge-token 部署之后的ID
const COIN_SYMBOL: &str = "Forge";
const COIN_NAME: &str = "Alkanes Forge";

const STOKEN_TEMPLATE_ID: u128 = 8; // forge-stoken 部署之后的ID

// BRC20 token configuration
const BRC20_TOKEN_NAME: &str = "FMAP";

// Custom error types for better error handling
#[derive(Debug, thiserror::Error)]
pub enum StakingPoolError {
    #[error("Mining has not started yet")]
    MiningNotStarted,
    #[error("Mining period has ended")]
    MiningEnded,
    #[error("Stake value is too low (minimum: {})", MIN_STAKE_VALUE)]
    InsufficientStakeValue,
    #[error("Invalid staking transaction")]
    InvalidStakingTransaction,
    #[error("Caller is not a staker")]
    CallerNotStaker,
    #[error("Already unstaking")]
    AlreadyUnstaking,
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("Storage operation failed: {0}")]
    StorageError(String),
    #[error("Serialization failed: {0}")]
    SerializationError(String),
    #[error("Calculation error: {0}")]
    CalculationError(String),
}

/// Main staking pool contract structure
/// Handles staking operations, profit calculations, and token management
#[derive(Default)]
pub struct StakingPool(());

/// Implementation of AlkaneResponder trait for the staking pool
impl AlkaneResponder for StakingPool {}

/// Message types for contract interaction
/// Defines all available operations that can be performed on the contract
#[derive(MessageDispatch)]
enum StakingPoolMessage {
    /// Initialize the contract and deploy the reward token
    #[opcode(0)]
    Initialize,

    /// Stake tokens to participate in mining
    #[opcode(50)]
    Stake,

    /// Unstake tokens and exit mining
    #[opcode(51)]
    Unstake,

    /// Get profit calculation for a specific staking position
    #[opcode(53)]
    #[returns(String)]
    GetProfit { index: u128, height: u128 },

    /// Claim accumulated rewards
    #[opcode(54)]
    Claim,

    /// Get the contract name
    #[opcode(99)]
    #[returns(String)]
    GetName,

    /// Get the contract symbol
    #[opcode(100)]
    #[returns(String)]
    GetSymbol,

    /// Get the total supply
    #[opcode(101)]
    #[returns(u128)]
    GetTotalSupply,

    /// Get the collection identifier
    #[opcode(998)]
    #[returns(String)]
    GetCollectionIdentifier,

    /// Get data for a specific staking position
    #[opcode(1000)]
    #[returns(Vec<u8>)]
    GetData { index: u128 },

    /// Get attributes for a specific staking position
    #[opcode(1002)]
    #[returns(String)]
    GetAttributes { index: u128 },

    /// Get the reward token's Alkane ID
    #[opcode(1003)]
    #[returns(String)]
    GetCoinAlkanesId,

    /// Get the token balance
    #[opcode(1004)]
    #[returns(String)]
    GetCoinBalance,
}

/// Implementation of Token trait for the staking pool
impl Token for StakingPool {
    /// Returns the name of the staking pool
    fn name(&self) -> String {
        String::from(CONTRACT_NAME)
    }

    /// Returns the symbol of the staking pool
    fn symbol(&self) -> String {
        String::from(CONTRACT_SYMBOL)
    }
}

/// Encodes a string into two u128 values for storage
/// 
/// # Arguments
/// * `s` - The string to encode
/// 
/// # Returns
/// * `(u128, u128)` - Two u128 values representing the encoded string
pub fn encode_string_to_u128(s: &str) -> (u128, u128) {
    let mut bytes = s.as_bytes().to_vec();
    
    // Ensure the string is exactly 32 bytes
    if bytes.len() < 32 {
        bytes.resize(32, 0); // Fill missing bytes with 0
    } else if bytes.len() > 32 {
        bytes.truncate(32); // Truncate excess bytes
    }

    // Split into two 16-byte blocks and convert to u128 (little endian)
    let (first_half, second_half) = bytes.split_at(16);
    let u1 = u128::from_le_bytes(first_half.try_into().unwrap());
    let u2 = u128::from_le_bytes(second_half.try_into().unwrap());

    (u1, u2)
}

/// Converts a staking period to a weight multiplier
/// 
/// # Arguments
/// * `period` - The staking period in days
/// 
/// # Returns
/// * `Decimal` - The weight multiplier for the period
pub fn period_to_weight_multiplier(period: u16) -> Decimal {
    match period {
        30 => Decimal::from_str("1.0").unwrap(),
        90 => Decimal::from_str("1.5").unwrap(),
        180 => Decimal::from_str("1.8").unwrap(),
        360 => Decimal::from_str("2.2").unwrap(),
        _ => Decimal::from_str("1.0").unwrap(), // Default weight for unknown periods
    }
}

impl StakingPool {
    /// Initialize the staking pool contract
    /// 
    /// Sets up all necessary storage values and deploys the reward token
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Success or failure of initialization
    fn initialize(&self) -> Result<CallResponse> {
        self.observe_initialization()?;

        // Add BRC20 token name
        self.add_brc20_token_name(BRC20_TOKEN_NAME)?;

        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Deploy the reward token contract
        self.deploy_reward_token()?;

        // Add auth token for contract operations
        response.alkanes.0.push(AlkaneTransfer {
            id: context.myself.clone(),
            value: 1u128,
        });

        Ok(response)
    }

    /// Deploy the reward token contract
    /// 
    /// # Returns
    /// * `Result<AlkaneTransfer>` - The deployed token transfer or error
    fn deploy_reward_token(&self) -> Result<AlkaneTransfer> {
        let (name_part1, name_part2) = encode_string_to_u128(COIN_NAME);
        let (symbol, _) = encode_string_to_u128(COIN_SYMBOL);
        
        let cellpack = Cellpack {
            target: AlkaneId {
                block: 5,
                tx: COIN_TEMPLATE_ID,
            },
            inputs: vec![0x0, TOKEN_CAP, name_part1, name_part2, symbol],
        };

        let sequence = self.sequence();
        let response = self.call(&cellpack, &AlkaneTransferParcel::default(), self.fuel())
            .map_err(|e| StakingPoolError::StorageError(format!("Failed to deploy token: {}", e)))?;

        let coin_id = AlkaneId {
            block: 2,
            tx: sequence,
        };

        self.set_coin_id(&coin_id);

        if response.alkanes.0.is_empty() {
            Err(StakingPoolError::StorageError("Token not returned from factory".to_string()).into())
        } else {
            Ok(response.alkanes.0[0].clone())
        }
    }

    /// Stake tokens to participate in mining
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Success or failure of staking operation
    fn stake(&self) -> Result<CallResponse> {
        self.verify_owner_authentication()?;

        let mut staking = Staking::from_tx(self.transaction())
            .map_err(|_| StakingPoolError::InvalidStakingTransaction)?;

        // Validate staking parameters
        self.validate_staking_parameters(&staking)?;

        let staking_index = self.get_next_staking_index();

        // Set invite relationship
        let invite_alkanes_id = AlkaneId {
            block: staking.alkanes_id[0],
            tx: staking.alkanes_id[1],
        };
        staking.invite_index = self.get_staking_index_by_id(&invite_alkanes_id);

        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Create staking position
        let cellpack = Cellpack {
            target: AlkaneId {
                block: 5,
                tx: STOKEN_TEMPLATE_ID,
            },
            inputs: vec![0x0, staking_index],
        };
        
        let sequence = self.sequence();
        let subresponse = self.call(&cellpack, &AlkaneTransferParcel::default(), self.fuel())
            .map_err(|e| StakingPoolError::StorageError(format!("Failed to create staking position: {}", e)))?;
        
        staking.alkanes_id = [2, sequence];

        // Add staking to storage
        self.add_staking_position(staking_index, &staking)?;

        if subresponse.alkanes.0.is_empty() {
            Err(StakingPoolError::StorageError("Staking position token not returned".to_string()).into())
        } else {
            response.alkanes.0.push(subresponse.alkanes.0[0].clone());
            Ok(response)
        }
    }

    //不依赖中间状态的算法，两种可以对比验证
    /// Validate staking parameters
    /// 
    /// # Arguments
    /// * `staking` - The staking data to validate
    /// 
    /// # Returns
    /// * `Result<()>` - Success or validation error
    fn validate_staking_parameters(&self, staking: &Staking) -> Result<()> {
        if staking.staking_height < MINING_FIRST_HEIGHT {
            return Err(StakingPoolError::MiningNotStarted.into());
        }
        
        if staking.staking_height > MINING_LAST_HEIGHT {
            return Err(StakingPoolError::MiningEnded.into());
        }
        
        if staking.staking_value < MIN_STAKE_VALUE as u128 {
            return Err(StakingPoolError::InsufficientStakeValue.into());
        }

        Ok(())
    }

    /// Get the next available staking index
    /// 
    /// # Returns
    /// * `u128` - The next staking index
    fn get_next_staking_index(&self) -> u128 {
        self.get_orbital_count().checked_add(1).unwrap_or(1)
    }

    /// Get staking index by Alkane ID
    /// 
    /// # Arguments
    /// * `alkane_id` - The Alkane ID
    /// 
    /// # Returns
    /// * `u128` - The staking index
    fn get_staking_index_by_id(&self, alkane_id: &AlkaneId) -> u128 {
        self.staking_id2index_pointer(alkane_id).get_value::<u128>()
    }

    /// Calculate profit using the standard algorithm
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// * `height` - The current block height
    /// 
    /// # Returns
    /// * `Result<(u128, u128, u128)>` - (total_profit, released_profit, withdrawn_amount)
    pub fn calc_profit(&self, index: u128, height: u128) -> Result<(u128, u128, u128)> {
        let curr_staking = self.get_staking(index);
        let mut start_height = curr_staking.staking_height;
        let end_height = curr_staking.get_mining_end_height(height as u64);
        let staking_weight = Decimal::from(curr_staking.staking_value) * period_to_weight_multiplier(curr_staking.period);
        let release_rate = Decimal::from(1) / Decimal::from(PROFIT_RELEASE_HEIGHT);
        let profit_factor = staking_weight * Decimal::from(MINING_ONE_BLOCK_VOLUME);
        let release_end = curr_staking.get_release_end_height(height as u64);

        let mut total_profit = Decimal::from(0);
        let mut total_released = Decimal::from(0);
        
        while start_height < end_height {
            let block_profit = profit_factor / self.get_staking_weight(start_height);
            total_profit += block_profit;
            
            let blocks_until_release = release_end - start_height - 1; // Release starts from next block
            let released_amount = if blocks_until_release >= PROFIT_RELEASE_HEIGHT {
                block_profit
            } else {
                block_profit * release_rate * Decimal::from(blocks_until_release)
            };
            total_released += released_amount;
            start_height += 1;
        }

        Ok((
            total_profit.floor().try_into().unwrap_or(0),
            total_released.floor().try_into().unwrap_or(0),
            curr_staking.withdraw_coin_value,
        ))
    }

    /// Get profit information for a staking position
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// * `height` - The current block height
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - JSON response with profit data
    fn get_profit(&self, index: u128, height: u128) -> Result<CallResponse> {
        let (total_profit, released_profit, withdrawn_amount) = self.calc_profit(index, height)?;
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        
        let profit_data = serde_json::to_vec(&[
            total_profit.to_string(), 
            released_profit.to_string(), 
            withdrawn_amount.to_string()
        ]).map_err(|e| StakingPoolError::SerializationError(format!("Failed to serialize profit data: {}", e)))?;
        
        response.data = profit_data;
        Ok(response)
    }

    /// Unstake tokens and exit mining
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Success or failure of unstaking operation
    fn unstake(&self) -> Result<CallResponse> {
        let context = self.context()?;

        let caller_index = self.get_staking_index_by_id(&context.caller);
        if caller_index == 0 {
            return Err(StakingPoolError::CallerNotStaker.into());
        }

        self.process_unstake(caller_index)?;
        let response = CallResponse::forward(&context.incoming_alkanes);
        Ok(response)
    }

    /// Claim accumulated rewards
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Success or failure of claim operation
    fn claim(&self) -> Result<CallResponse> {
        let context = self.context()?;

        let caller_index = self.get_staking_index_by_id(&context.caller);
        if caller_index == 0 {
            return Err(StakingPoolError::CallerNotStaker.into());
        }

        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let (_, released_profit, withdrawn_amount) = self.calc_profit(caller_index, self.height() as u128)?;
        let claimable_amount = released_profit.saturating_sub(withdrawn_amount);
        
        if claimable_amount > 0 {
            response.alkanes.0.push(AlkaneTransfer {
                id: self.get_coin_id(),
                value: claimable_amount,
            });
            
            let mut staking = self.get_staking(caller_index);
            staking.withdraw_coin_value += claimable_amount;
            self.set_staking(caller_index, &staking);
        }

        Ok(response)
    }

    /// Verify that the caller is the contract owner using collection token
    ///
    /// # Returns
    /// * `Result<()>` - Success or error if not owner
    fn verify_owner_authentication(&self) -> Result<()> {
        let context = self.context()?;

        if context.incoming_alkanes.0.len() != 1 {
            return Err(StakingPoolError::AuthenticationFailed("Did not authenticate with only the auth token".to_string()).into());
        }

        let transfer = &context.incoming_alkanes.0[0];
        if transfer.id != context.myself {
            return Err(StakingPoolError::AuthenticationFailed("Supplied alkane is not auth token".to_string()).into());
        }

        if transfer.value < 1 {
            return Err(StakingPoolError::AuthenticationFailed("Less than 1 unit of auth token supplied".to_string()).into());
        }

        Ok(())
    }

    // Storage management methods

    /// Get coin ID storage pointer
    fn coin_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/coin_id")
    }

    /// Set the coin ID
    /// 
    /// # Arguments
    /// * `id` - The Alkane ID to set
    pub fn set_coin_id(&self, id: &AlkaneId) {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&id.block.to_le_bytes());
        bytes.extend_from_slice(&id.tx.to_le_bytes());
        self.coin_id_pointer().set(Arc::new(bytes));
    }

    /// Get the coin ID
    /// 
    /// # Returns
    /// * `AlkaneId` - The stored coin ID
    pub fn get_coin_id(&self) -> AlkaneId {
        let bytes = self.coin_id_pointer().get();
        if bytes.len() >= 32 {
            AlkaneId {
                block: u128::from_le_bytes(bytes[0..16].try_into().unwrap_or([0; 16])),
                tx: u128::from_le_bytes(bytes[16..32].try_into().unwrap_or([0; 16])),
            }
        } else {
            AlkaneId::default()
        }
    }

    /// Get BRC20 token count storage pointer
    fn brc20_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/brc20_count")
    }

    /// Get the BRC20 token count
    /// 
    /// # Returns
    /// * `u8` - The number of BRC20 tokens
    pub fn get_brc20_count(&self) -> u8 {
        self.brc20_count_pointer().get_value::<u8>()
    }

    /// Set the BRC20 token count
    /// 
    /// # Arguments
    /// * `count` - The count to set
    fn set_brc20_count(&self, count: u8) {
        self.brc20_count_pointer().set_value(count)
    }

    /// Get BRC20 token name storage pointer
    fn brc20_name_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/brc20_names")
    }

    /// Add a BRC20 token name
    /// 
    /// # Arguments
    /// * `name` - The token name to add
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    fn add_brc20_token_name(&self, name: &str) -> Result<()> {
        let index = self.get_brc20_count();
        self.brc20_name_pointer()
            .select_index(index as u32)
            .set(Arc::new(name.to_string().into_bytes()));
        
        let new_count = index.checked_add(1)
            .ok_or_else(|| StakingPoolError::StorageError("BRC20 count overflow".to_string()))?;
        self.set_brc20_count(new_count);
        
        Ok(())
    }

    /// Get orbital count storage pointer
    fn orbital_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/orbital_count")
    }

    /// Get the orbital count
    /// 
    /// # Returns
    /// * `u128` - The number of orbitals
    pub fn get_orbital_count(&self) -> u128 {
        self.orbital_count_pointer().get_value::<u128>()
    }

    /// Set the orbital count
    /// 
    /// # Arguments
    /// * `count` - The count to set
    fn set_orbital_count(&self, count: u128) {
        self.orbital_count_pointer().set_value(count)
    }

    /// Get staking position storage pointer
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `StoragePointer` - The storage pointer
    fn staking_pointer(&self, index: u128) -> StoragePointer {
        StoragePointer::from_keyword("/staking/").select(&index.to_le_bytes().to_vec())
    }

    /// Get staking ID to index mapping storage pointer
    /// 
    /// # Arguments
    /// * `alkane_id` - The Alkane ID
    /// 
    /// # Returns
    /// * `StoragePointer` - The storage pointer
    fn staking_id2index_pointer(&self, alkane_id: &AlkaneId) -> StoragePointer {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&alkane_id.block.to_le_bytes());
        bytes.extend_from_slice(&alkane_id.tx.to_le_bytes());
        StoragePointer::from_keyword("/staking/id2index/").select(&bytes)
    }

    /// Add a staking position
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// * `staking` - The staking data
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn add_staking_position(&self, index: u128, staking: &Staking) -> Result<()> {
        // Store staking data
        let serialized = Staking::serialize(staking)
            .map_err(|e| StakingPoolError::SerializationError(format!("Failed to serialize staking: {}", e)))?;
        self.staking_pointer(index).set(Arc::new(serialized));
        
        // Set ID to index mapping
        self.staking_id2index_pointer(&staking.get_alanes_id()).set_value(index);
        
        // Set invite relationship
        self.set_invite_relationship(index, staking.invite_index);
        
        // Update weights
        let staking_weight = Decimal::from(staking.staking_value) * period_to_weight_multiplier(staking.period);
        
        let current_weight = self.get_staking_weight(staking.staking_height);
        self.set_staking_weight(staking.staking_height, current_weight + staking_weight);
        
        let current_expire_weight = self.get_staking_expire(staking.get_expire_height());
        self.set_staking_expire(staking.get_expire_height(), current_expire_weight + staking_weight);
        
        // Update orbital count
        self.set_orbital_count(index);
        
        Ok(())
    }

    /// Process unstaking for a position
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    fn process_unstake(&self, index: u128) -> Result<()> {
        let mut staking = self.get_staking(index);
        
        if staking.unstaking_height > 0 {
            return Err(StakingPoolError::AlreadyUnstaking.into());
        }
        
        staking.unstaking_height = self.height();
        
        let serialized = Staking::serialize(&staking)
            .map_err(|e| StakingPoolError::SerializationError(format!("Failed to serialize unstaking: {}", e)))?;
        self.staking_pointer(index).set(Arc::new(serialized));
        
        if staking.get_expire_height() <= self.height() {
            return Ok(());
        }

        // Update weights
        let staking_weight = Decimal::from(staking.staking_value) * period_to_weight_multiplier(staking.period);
        
        let current_weight = self.get_staking_weight(staking.unstaking_height);
        self.set_staking_weight(staking.unstaking_height, current_weight - staking_weight);
        
        let current_expire_weight = self.get_staking_expire(staking.get_expire_height());
        self.set_staking_expire(staking.get_expire_height(), current_expire_weight - staking_weight);

        Ok(())
    }

    /// Get staking data by index
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Staking` - The staking data
    pub fn get_staking(&self, index: u128) -> Staking {
        let data = self.staking_pointer(index).get();
        Staking::descrialize(&data).unwrap_or_default()
    }

    /// Set staking data by index
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// * `staking` - The staking data to set
    fn set_staking(&self, index: u128, staking: &Staking) {
        if let Ok(serialized) = Staking::serialize(staking) {
            self.staking_pointer(index).set(Arc::new(serialized));
        }
    }

    /// Get staking data by Alkane ID
    /// 
    /// # Arguments
    /// * `alkane_id` - The Alkane ID
    /// 
    /// # Returns
    /// * `Staking` - The staking data
    fn get_staking_by_id(&self, alkane_id: &AlkaneId) -> Staking {
        let index = self.get_staking_index_by_id(alkane_id);
        self.get_staking(index)
    }

    /// Get invite relationship storage pointer
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `StoragePointer` - The storage pointer
    fn staking_invite_pointer(&self, index: u128) -> StoragePointer {
        StoragePointer::from_keyword("/staking/share/").select(&index.to_le_bytes().to_vec())
    }

    /// Get invited staking positions
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Vec<Staking>` - The invited staking positions
    fn get_invited_stakings(&self, index: u128) -> Vec<Staking> {
        let data = self.staking_invite_pointer(index).get();
        let indices = Staking::descrialize_invite_vec(&data).unwrap_or_default();
        indices.iter().map(|&idx| self.get_staking(idx)).collect()
    }

    /// Get invited indices
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Vec<u128>` - The invited indices
    fn get_invited_indices(&self, index: u128) -> Vec<u128> {
        let data = self.staking_invite_pointer(index).get();
        Staking::descrialize_invite_vec(&data).unwrap_or_default()
    }

    /// Set invite relationship
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// * `invite_index` - The invite index
    fn set_invite_relationship(&self, index: u128, invite_index: u128) {
        if invite_index > 0 {
            let mut indices = self.get_invited_indices(invite_index);
            indices.push(index);
            
            if let Ok(serialized) = Staking::serialize_invite_vec(&indices) {
                self.staking_invite_pointer(invite_index).set(Arc::new(serialized));
            }
        }
    }

    /// Get staking weight storage pointer
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// 
    /// # Returns
    /// * `StoragePointer` - The storage pointer
    fn staking_weight_pointer(&self, height: u64) -> StoragePointer {
        StoragePointer::from_keyword("/staking_weight/").select(&height.to_le_bytes().to_vec())
    }

    /// Get staking expire storage pointer
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// 
    /// # Returns
    /// * `StoragePointer` - The storage pointer
    fn staking_expire_pointer(&self, height: u64) -> StoragePointer {
        StoragePointer::from_keyword("/staking_expire/").select(&height.to_le_bytes().to_vec())
    }

    /// Get staking expire weight
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// 
    /// # Returns
    /// * `Decimal` - The expire weight
    fn get_staking_expire(&self, height: u64) -> Decimal {
        let data = self.staking_expire_pointer(height).get();
        if !data.is_empty() {
            Staking::descrialize_decimal(&data).unwrap_or(Decimal::from(0))
        } else {
            Decimal::from(0)
        }
    }

    /// Set staking expire weight
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// * `value` - The weight value to set
    fn set_staking_expire(&self, height: u64, value: Decimal) {
        if let Ok(serialized) = Staking::serialize_decimal(&value) {
            self.staking_expire_pointer(height).set(Arc::new(serialized));
        }
    }

    /// Get staking weight
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// 
    /// # Returns
    /// * `Decimal` - The staking weight
    fn get_staking_weight(&self, height: u64) -> Decimal {
        let data = self.staking_weight_pointer(height).get();
        if !data.is_empty() {
            return Staking::descrialize_decimal(&data).unwrap_or(Decimal::from(0));
        }

        let expire_weight = self.get_staking_expire(height);
        let mut weight = Decimal::from(0) - expire_weight;
        let mut current_height = height;
        
        while current_height > MINING_FIRST_HEIGHT {
            current_height -= 1;
            let data = self.staking_weight_pointer(current_height).get();
            if !data.is_empty() {
                weight += Staking::descrialize_decimal(&data).unwrap_or(Decimal::from(0));
                break;
            } else {
                weight -= self.get_staking_expire(current_height);
            }
        }
        
        weight
    }

    /// Set staking weight
    /// 
    /// # Arguments
    /// * `height` - The block height
    /// * `weight` - The weight value to set
    pub fn set_staking_weight(&self, height: u64, weight: Decimal) {
        if let Ok(serialized) = Staking::serialize_decimal(&weight) {
            self.staking_weight_pointer(height).set(Arc::new(serialized));
        }
    }

    /// Get the contract name
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with contract name
    fn get_name(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.name().into_bytes();
        Ok(response)
    }

    /// Get the contract symbol
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with contract symbol
    fn get_symbol(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.symbol().into_bytes();
        Ok(response)
    }

    /// Get the total supply of tokens
    /// Returns the total number of orbital count
    fn get_total_supply(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Total supply is the orbital count
        response.data = self.get_orbital_count().to_le_bytes().to_vec();

        Ok(response)
    }

    /// Get the collection identifier
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with collection identifier
    fn get_collection_identifier(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let identifier = format!("{}:{}", context.myself.block, context.myself.tx);
        response.data = identifier.into_bytes();
        Ok(response)
    }

    /// Get data for a specific staking position
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with position data
    pub fn get_data(&self, index: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let response = CallResponse::forward(&context.incoming_alkanes);
        // TODO: Implement position data retrieval
        Ok(response)
    }

    /// Get attributes for a specific staking position
    /// 
    /// # Arguments
    /// * `index` - The staking position index
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with position attributes
    pub fn get_attributes(&self, index: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let staking = self.get_staking(index);
        
        let attributes = serde_json::to_vec(&staking)
            .map_err(|e| StakingPoolError::SerializationError(format!("Failed to serialize attributes: {}", e)))?;
        response.data = attributes;
        Ok(response)
    }

    /// Get the reward token's Alkane ID
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with token ID
    pub fn get_coin_alkanes_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let alkane_id = self.get_coin_id();
        let id_string = format!("{}:{}", alkane_id.block, alkane_id.tx);
        response.data = id_string.into_bytes();
        Ok(response)
    }

    /// Get the token balance
    /// 
    /// # Returns
    /// * `Result<CallResponse>` - Response with balance
    pub fn get_coin_balance(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let alkane_id = self.get_coin_id();
        let balance = self.balance(&context.myself, &alkane_id);
        response.data = balance.to_string().into_bytes();
        Ok(response)
    }

    /// Set storage value (utility function)
    /// 
    /// # Arguments
    /// * `key` - The storage key
    /// * `value` - The value to store
    pub fn set_storage(&self, key: Vec<u8>, value: Vec<u8>) {
        StoragePointer::wrap(&key).set(Arc::new(value));
    }
}

declare_alkane! {
    impl AlkaneResponder for StakingPool {
        type Message = StakingPoolMessage;
    }
}
