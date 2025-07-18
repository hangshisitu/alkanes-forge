use std::result::Result::Ok;
use alkanes_runtime::{
    declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, storage::StoragePointer,
    token::Token,
};
use bitcoin::absolute::Height;
use bitcoin::transaction::IndexOutOfBoundsError;
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;
use metashrew_support::utils::{consume_exact, consume_sized_int, consume_to_end};

use alkanes_support::{
    cellpack::Cellpack,
    id::AlkaneId,
    parcel::{AlkaneTransfer, AlkaneTransferParcel},
    response::CallResponse,
    witness::find_witness_payload,
};

use anyhow::{anyhow, Result};
use bitcoin::{Block, Transaction, TxOut};
use metashrew_support::utils::consensus_decode;
use types_support::staking;
use std::f32::consts::E;
use std::io::Cursor;
use std::sync::Arc;
use types_support::{
    staking::Staking,
    staking::StakingStat,
};
use std::cmp::{max, min};
use rust_decimal::Decimal;
use std::str::FromStr;

const ALKANE_BG_ID: AlkaneId = AlkaneId {
    block: 2,
    tx: 31060,
};

const CONTRACT_NAME: &str = "Staking Pool";
const CONTRACT_SYMBOL: &str = "SP";
const WHITELIST_MINT_START_TM: u64 = 902536;
const PUBLIC_MINT_START_TM: u64 = 902566;


const CAP: u128 = 100000000000000000;
const MINING_CAP: u64 = 52000000000000000;
const MINING_ONE_BLOCK_VOLUME: u64 = 1003086419753;
const MINING_ONE_DAY_VOLUME: u64 = 144444444444444;
const MINING_FIRST_HEIGHT: u64 = 450;  //挖矿的第一个块高度
const MINING_LAST_HEIGHT: u64 = MINING_FIRST_HEIGHT + 144*360-1; //挖矿的最后块高度
const MIN_STAKING_VALUE: u64 = 1000;
const PROFIT_RELEASE_HEIGHT: u64 = 144*180;
const PROFIT_RELEASE_DAY: u64 = 180;

const COIN_TEMPLATE_ID: u128 = 3; //TODO 部署代码后得到模板ID
const COIN_SYMBOL: &str = "forge";
const COIN_NAME: &str = "Alkanes Forge";

const ORBITAL_TEMPLATE_ID: u128 = 1;

const BRC20_NAME_0: &str = "sats";


/// Collection Contract Structure
/// This is the main contract structure that implements the NFT collection functionality
#[derive(Default)]
pub struct StakingPool(());

/// Implementation of AlkaneResponder trait for the collection
impl AlkaneResponder for StakingPool {}

/// Message types for contract interaction
/// These messages define the available operations that can be performed on the contract
#[derive(MessageDispatch)]
enum StakingPoolMessage {
    /// Initialize the contract and perform premine
    #[opcode(0)]
    Initialize,

    #[opcode(50)]
    Staking,

    #[opcode(51)]
    Unstaking,

    #[opcode(53)]
    #[returns(String)]
    GetProfit{
        index: u128,
        height: u128
    },

    #[opcode(54)]
    Claim,

    /// Get the name of the collection
    #[opcode(99)]
    #[returns(String)]
    GetName,

    /// Get the symbol of the collection
    #[opcode(100)]
    #[returns(String)]
    GetSymbol,

    // /// Get the total supply (minted + premine)
    // #[opcode(101)]
    // #[returns(u128)]
    // GetTotalSupply,

    // /// Get the total count of orbitals
    // #[opcode(102)]
    // #[returns(u128)]
    // GetOrbitalCount,

    // /// Get the minted count of orbitals
    // #[opcode(103)]
    // #[returns(u128)]
    // GetOrbitalMinted,


    /// Get the collection identifier
    #[opcode(998)]
    #[returns(String)]
    GetCollectionIdentifier,

    /// Get PNG data for a specific orbital
    ///
    /// # Arguments
    /// * `index` - The index of the orbital
    #[opcode(1000)]
    #[returns(Vec<u8>)]
    GetData { index: u128 },

    /// Get attributes for a specific orbital
    ///
    /// # Arguments
    /// * `index` - The index of the orbital
    #[opcode(1002)]
    #[returns(String)]
    GetAttributes { index: u128 },

    #[opcode(1003)]
    #[returns(String)]
    GetCoinAlkanesId,

    #[opcode(1004)]
    #[returns(String)]
    GetBalance,

}

/// Implementation of Token trait
impl Token for StakingPool {
    /// Returns the name of the token collection
    fn name(&self) -> String {
        String::from(CONTRACT_NAME)
    }

    /// Returns the symbol of the token collection
    fn symbol(&self) -> String {
        String::from(CONTRACT_SYMBOL)
    }
}

pub fn encode_string_to_u128(s: &str) -> (u128, u128) {
    // Make sure the string is 32 bytes long
    let mut bytes = s.as_bytes().to_vec();
    if bytes.len() < 32 {
        bytes.resize(32, 0); //Fill the missing part with 0
    } else if bytes.len() > 32 {
        bytes.truncate(32); // Cut off the excess part
    }

    // Split into two 16-byte blocks and convert to u128 (big endian)
    let (first_half, second_half) = bytes.split_at(16);
    let u1 = u128::from_le_bytes(first_half.try_into().unwrap());
    let u2 = u128::from_le_bytes(second_half.try_into().unwrap());

    (u1, u2)
}

pub fn period_to_w(period: u16) -> Decimal {
    match period {
        30 => Decimal::from_str("1.0").unwrap(),
        90 => Decimal::from_str("1.5").unwrap(),
        180 => Decimal::from_str("1.8").unwrap(),
        360 => Decimal::from_str("2.2").unwrap(),
        _ => Decimal::from_str("1.0").unwrap(),
    }
}

impl StakingPool {
    /// Initialize the contract
    ///
    /// initializes all necessary storage values
    ///
    /// # Returns
    /// * `Result<CallResponse>` - Success or failure of initialization
    fn initialize(&self) -> Result<CallResponse> {
        self.observe_initialization()?;

        self.add_brc20_name(BRC20_NAME_0);

        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        //部署coin合约，所有coin属于质押池合约
        self.deploy_coin_token()?;

        // Collection token acts as auth token for contract minting without any limits
        response.alkanes.0.push(AlkaneTransfer {
            id: context.myself.clone(),
            value: 1u128,
        });

        Ok(response)
    }

    fn deploy_coin_token(&self) -> Result<AlkaneTransfer> {
        let (name_part1,name_part2) = encode_string_to_u128(COIN_NAME);
        let (symbol,_) = encode_string_to_u128(COIN_SYMBOL);
        let cellpack = Cellpack {
            target: AlkaneId {
                block: 5,
                tx: COIN_TEMPLATE_ID,
            },
            inputs: vec![0x0, CAP,name_part1,name_part2,symbol],
        };

        let sequence = self.sequence();
        let response = self.call(&cellpack, &AlkaneTransferParcel::default(), self.fuel())?;

        let coin_id = AlkaneId {
            block: 2,
            tx: sequence,
        };

        self.set_coin_id(&coin_id);

        if response.alkanes.0.len() < 1 {
            Err(anyhow!("orbital token not returned with factory"))
        } else {
            Ok(response.alkanes.0[0])
        }
    }
    
    fn staking(&self) -> Result<CallResponse> {

        self.only_owner()?;

        let Ok(mut staking) = Staking::from_tx(self.transaction()) else {
            return Err(anyhow!("invalid staking transaction"));
        };

        // TODO staking_height  小于当前高度，且是严格递增
        if staking.staking_height < MINING_FIRST_HEIGHT{
            return Err(anyhow!("Not yet started"));
        }else if staking.staking_height > MINING_LAST_HEIGHT{
            return Err(anyhow!("Mining ended"));
        }
        if staking.staking_value < MIN_STAKING_VALUE as u128{
            return Err(anyhow!("Not enough value"));
        }

        let index = self.get_orbital_count().checked_add(1).unwrap();

        let invite_alkanes_id = AlkaneId{block:staking.alkanes_id[0],tx:staking.alkanes_id[1]};
        staking.invite_index = self.staking_id2index_pointer(&invite_alkanes_id).get_value::<u128>();

        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let cellpack = Cellpack {
            target: AlkaneId {
                block: 5,
                tx: ORBITAL_TEMPLATE_ID,
            },
            inputs: vec![0x0, index],
        };
        let sequence = self.sequence();
        let subresponse = self.call(&cellpack, &AlkaneTransferParcel::default(), self.fuel())?;
        staking.alkanes_id = [2,sequence];

        self.add_staking(index,&staking);

        if subresponse.alkanes.0.len() < 1 {
            Err(anyhow!("orbital token not returned with factory"))
        } else {
            response.alkanes.0.push(subresponse.alkanes.0[0].clone());
            Ok(response)
        }
    
    }


    //不依赖中间状态的算法，两种可以对比验证
    fn calc_profit_1(&self,index:u128,height:u128) -> Result<(u128,u128,u128)>{ 
        let count  = self.get_orbital_count();
        let curr_staking = self.get_staking(index);
        let start = self.height_to_no(curr_staking.staking_height);
        let end: u64 = self.height_to_no(curr_staking.get_mining_end_height(height as u64));
        let  c  = Decimal::from(curr_staking.staking_value)
        .checked_mul(Decimal::from(end-start)).unwrap()
        .checked_mul(Decimal::from(period_to_w(curr_staking.period))).unwrap();

        let mut pre_v =vec![Decimal::from(0);(end-start) as usize];

        let mut v = Decimal::from(0);
        for i in 0..count{
            let staking = self.get_staking(i+1);
            let t_s = self.height_to_no(staking.staking_height);
            let t_e = self.height_to_no(staking.get_mining_end_height( height as u64));
            let length = max(min(t_e,end)-max(t_s,start),0);
            if length == 0{
                continue;
            }
            v = v.checked_add(period_to_w(staking.period).checked_mul(Decimal::from(staking.staking_value.checked_mul(length as u128).unwrap())).unwrap()).unwrap() ;

            let mut cross_s = max(t_s,start);
            let cross_e = min(t_e,end);

            //计算每个快质押量
            while cross_s < cross_e {
                let t = (cross_s -start) as usize;
                pre_v[t] = pre_v[t].checked_add(period_to_w(staking.period).checked_mul(Decimal::from(staking.staking_value)).unwrap()).unwrap();
                cross_s +=1;
            }
        }
        let p = if v > Decimal::from(0) {
            c.checked_div(v).unwrap().checked_mul(Decimal::from(MINING_ONE_DAY_VOLUME)).unwrap().checked_mul(Decimal::from(end-start)).unwrap()
        }else{
            Decimal::from(0)
        };

        let curr_staking_w = Decimal::from(curr_staking.staking_value).checked_mul(period_to_w(curr_staking.period)).unwrap();
        //计算每个快收益
        pre_v.iter_mut().for_each(|v| *v = curr_staking_w.checked_div(*v).unwrap().checked_mul(Decimal::from(MINING_ONE_DAY_VOLUME)).unwrap());

        let release_end = self.height_to_no(curr_staking.get_release_end_height(height as u64));
        //计算释放收益
        let rate = Decimal::from(1) / Decimal::from(PROFIT_RELEASE_DAY);
        let release_p: Decimal = pre_v.iter().enumerate().map(|(i,v)| {
            let cnt = release_end.checked_sub(i as u64 + start +1).unwrap();
            if cnt >= PROFIT_RELEASE_DAY {
                *v
            } else {
                v.checked_mul(rate).unwrap().checked_mul(Decimal::from(cnt)).unwrap()
            }
        }).sum();
    
        return Ok((p.floor().try_into().unwrap(),release_p.floor().try_into().unwrap(),curr_staking.withdraw_coin_value));
    }

    fn calc_profit(&self,index:u128,height:u128) -> Result<(u128,u128,u128)>{
        let curr_staking = self.get_staking(index);
        let mut start = self.height_to_no(curr_staking.staking_height);
        let end = self.height_to_no(curr_staking.get_mining_end_height(height as u64));
        let curr_staking_w = Decimal::from(curr_staking.staking_value) * period_to_w(curr_staking.period);
        let rate =Decimal::from(1) / Decimal::from(PROFIT_RELEASE_DAY);
        let factor = curr_staking_w * Decimal::from(MINING_ONE_DAY_VOLUME);
        let release_end = self.height_to_no(curr_staking.get_release_end_height(height as u64));

        let mut total_p = Decimal::from(0); 
        let mut total_r = Decimal::from(0);
        while start < end{ 
            let p = factor / self.get_staking_weight(start);
            total_p += p;
            let cnt = release_end-start-1; //下个块开始释放
            let r = if cnt >= PROFIT_RELEASE_DAY {
                p
            } else {
                p * rate * Decimal::from(cnt)
            };
            total_r += r;
            start += 1;
        }

        Ok((total_p.floor().try_into()?,
            total_r.floor().try_into()?,
            curr_staking.withdraw_coin_value))
    }

    fn get_profit(&self,index:u128,height:u128) ->Result<CallResponse> { 
        let (p,r,w) = self.calc_profit(index,height)?;
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = serde_json::to_vec(&[p.to_string(),r.to_string(),w.to_string()]).unwrap();
        Ok(response)
    }

    fn test_json(&self,p:u128,r:u128,w:u128) -> Vec<u8>{
        serde_json::to_vec(&[p.to_string(),r.to_string(),w.to_string()]).unwrap()
    }

    fn unstaking(&self) -> Result<CallResponse> { 
        let context = self.context()?;

        let caller_index = self.staking_id2index_pointer(&context.caller).get_value::<u128>();
        if caller_index == 0 {
            return Err(anyhow!("caller is not staking"));
        }

        self.staking_unstaking(caller_index)?;
        let response = CallResponse::forward(&context.incoming_alkanes);
        Ok(response)
    }

    fn claim(&self) -> Result<CallResponse> { 
        let context = self.context()?;

        let caller_index = self.staking_id2index_pointer(&context.caller).get_value::<u128>();
        if caller_index == 0 {
            return Err(anyhow!("caller is not staking"));
        }

        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let (_,r,w) = self.calc_profit(caller_index,self.height() as u128)?;
        if r>w {
            response.alkanes.0.push(AlkaneTransfer {
                id: self.get_coin_id(),
                value: r-w,
            });
            let mut staking = self.get_staking(caller_index);
            staking.withdraw_coin_value += r-w;
            self.set_staking(caller_index, &staking);
        }

        
        Ok(response)
    }

    /// Verify that the caller is the contract owner using collection token
    ///
    /// # Returns
    /// * `Result<()>` - Success or error if not owner
    fn only_owner(&self) -> Result<()> {
        let context = self.context()?;

        if context.incoming_alkanes.0.len() != 1 {
            return Err(anyhow!(
                "did not authenticate with only the auth token"
            ));
        }

        let transfer = context.incoming_alkanes.0[0].clone();
        if transfer.id != context.myself.clone() {
            return Err(anyhow!("supplied alkane is not auth token"));
        }

        if transfer.value < 1 {
            return Err(anyhow!(
                "less than 1 unit of auth token supplied to authenticate"
            ));
        }

        Ok(())
    }

    ////////////////storage pointers///////////////////////////////////////
    /// 
    fn coin_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/coin_id")
    }

    pub fn set_coin_id(&self, id: &AlkaneId) {
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&id.block.to_le_bytes());
        bytes.extend_from_slice(&id.tx.to_le_bytes());
        self.coin_id_pointer().set(Arc::new(bytes));
    }

    pub fn get_coin_id(&self) -> AlkaneId {
        let bytes = self.coin_id_pointer().get();
        AlkaneId {
            block: u128::from_le_bytes(bytes[0..16].try_into().unwrap()),
            tx: u128::from_le_bytes(bytes[16..32].try_into().unwrap()),
        }
    }

    /// brc20 代币名字和index 存储
    fn brc20_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/brc20_count")
    }

    fn get_brc20_count(&self) -> u8 {
        self.brc20_count_pointer().get_value::<u8>()
    }

    fn set_brc20_count(&self, count: u8) {
        self.brc20_count_pointer().set_value(count)
    }

    fn brc20_name_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/brc20_names")
    }

    fn add_brc20_name(&self, name: &str) {
        let index = self.get_brc20_count();
        self.brc20_name_pointer().select_index(index as u32).set(Arc::new(name.to_string().into_bytes()));
        self.set_brc20_count(index.checked_add(1).expect("brc20 count overflow"));
    }

    fn get_brc20_name(&self,index:u8) -> String {
        let name = self.brc20_name_pointer().select_index(index as u32).get();
        String::from_utf8_lossy(&name).to_string()
    }

    ///  质押凭证存款
    fn orbital_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/orbital_count")
    }

    fn get_orbital_count(&self) -> u128 {
        self.orbital_count_pointer().get_value::<u128>()
    }

    fn set_orbital_count(&self, count: u128) {
        self.orbital_count_pointer().set_value(count)
    }

    fn staking_pointer(&self, index: u128) -> StoragePointer {
        StoragePointer::from_keyword("/staking/").select(&index.to_le_bytes().to_vec())
    }

    fn staking_id2index_pointer(&self,alkane_id: &AlkaneId) -> StoragePointer{
        let mut bytes = Vec::with_capacity(32);
        bytes.extend_from_slice(&alkane_id.block.to_le_bytes());
        bytes.extend_from_slice(&alkane_id.tx.to_le_bytes());
        //TODO字符串长度反而更短
        StoragePointer::from_keyword("/staking/id2index/").select(&bytes)
    }
    fn add_staking(&self,index: u128,staking: &Staking) {
        self.staking_pointer(index).set(Arc::new(Staking::serialize(staking).unwrap()));
        self.staking_id2index_pointer(&staking.get_alanes_id()).set_value(index);
        self.index_invite(index,staking.invite_index);
        let curr_w =  Decimal::from(staking.staking_value) * period_to_w(staking.period);

        let h_w = self.get_staking_weight(self.height_to_no(staking.staking_height));
        self.set_staking_weight(self.height_to_no(staking.staking_height), h_w + curr_w);
        let h_exp_w = self.get_staking_expire(self.height_to_no(staking.get_expire_height()));
        self.set_staking_expire(self.height_to_no(staking.get_expire_height()), h_exp_w + curr_w);


        // let mut stat = self.get_staking_stat(staking.staking_height);
        // stat.staking_weight += curr_w;
        // stat.init_weight += curr_w;
        // self.set_staking_stat(staking.staking_height,&stat);
        // let mut stat2 = self.get_staking_stat(staking.expire_height);
        // stat2.expire_weight += curr_w;
        // self.set_staking_stat(staking.expire_height, &stat2);
        self.set_orbital_count(index);
    }

    fn staking_unstaking(&self, index: u128) -> Result<()>{ 
        let mut staking = self.get_staking(index);
        if staking.unstaking_height>0 {
            return Err(anyhow!("already unstaking"));
        }
        staking.unstaking_height = self.height();
        self.staking_pointer(index).set(Arc::new(Staking::serialize(&staking).unwrap()));
        if staking.get_expire_height() <= self.height() {
            return Ok(());
        }

        let curr_w =  Decimal::from(staking.staking_value) * period_to_w(staking.period);
        let h_w = self.get_staking_weight(self.height_to_no(staking.unstaking_height));
        self.set_staking_weight(staking.unstaking_height, h_w - curr_w);
        let h_exp_w = self.get_staking_expire(self.height_to_no(staking.get_expire_height()));
        self.set_staking_expire(self.height_to_no(staking.get_expire_height()), h_exp_w - curr_w);

        Ok(())
        // let mut stat = self.get_staking_stat(staking.unstaking_height);
        // let curr_w =  Decimal::from(staking.staking_value) * period_to_w(staking.period);
        // stat.unstaking_weight += curr_w;
        // self.set_staking_stat(staking.unstaking_height,&stat);

        
        // let mut stat = self.get_staking_stat(staking.expire_height);
        // let curr_w =  Decimal::from(staking.staking_value) * period_to_w(staking.period);
        // stat.expire_weight -= curr_w;
        // self.set_staking_stat(staking.expire_height,&stat);

    }

    fn get_staking(&self, index: u128) -> Staking {
        let data = self.staking_pointer(index).get();
        Staking::descrialize(&data).unwrap()
    }

    fn set_staking(&self, index: u128, staking: &Staking) {
        self.staking_pointer(index).set(Arc::new(Staking::serialize(staking).unwrap()));
    }

    fn get_staking_by_id(&self, alkane_id: &AlkaneId) ->Staking {
        let index = self.staking_id2index_pointer(alkane_id).get_value::<u128>();
        self.get_staking(index)
    }

    //邀请关系存款
    fn staking_invite_pointer(&self,index: u128) -> StoragePointer{
        StoragePointer::from_keyword("/staking/share/").select(&index.to_le_bytes().to_vec())
    }

    fn get_invite_stakings(&self, index: u128) -> Vec<Staking> {
        let data = self.staking_invite_pointer(index).get();
        let indexs = Staking::descrialize_invite_vec(&data).unwrap();
        indexs.iter().map(|index| self.get_staking(*index)).collect()
    }

    fn get_invite_indexs(&self, index: u128) -> Vec<u128> {
        let data = self.staking_invite_pointer(index).get();
        Staking::descrialize_invite_vec(&data).unwrap()
    }

    //建立邀请关系索引
    fn index_invite(&self, index: u128, invite_index: u128){
        if invite_index>0 {
            let mut indexs = self.get_invite_indexs(invite_index);
            indexs.push(index);
            self.staking_invite_pointer(invite_index).set(
                Arc::new(Staking::serialize_invite_vec(&indexs).unwrap()));
        }
    }

    // fn staking_stat_pointer(&self, height: u64) -> StoragePointer {
    //     StoragePointer::from_keyword("/staking_stat").select(&height.to_le_bytes().to_vec())
    // }

    // fn get_staking_stat(&self, height: u64) -> StakingStat { 
    //     let data = self.staking_stat_pointer(height).get();
    //     if data.len()>0 {
    //         return StakingStat::descrialize(&data).unwrap();
    //     }
        
    //     let mut pre_stat = StakingStat::default();
    //     let mut h = height;
    //     while h > MINING_FIRST_HEIGHT {
    //         h -=1;
    //         let d = self.staking_stat_pointer(h ).get();
    //         if d.len()>0 {
    //             pre_stat = StakingStat::descrialize(&d).unwrap();
    //             break;
    //         }
    //     };
        
    //     StakingStat { 
    //         staking_weight: Decimal::from(0),
    //         unstaking_weight: Decimal::from(0), 
    //         expire_weight: Decimal::from(0),
    //         init_weight: pre_stat.total_weight(),
    //         weight: Decimal::from(0), }
    // }


    // fn set_staking_stat(&self, height: u64, stat: &StakingStat) {
    //     self.staking_stat_pointer(height).set(Arc::new(StakingStat::serialize(stat).unwrap()));
    // }

    fn staking_weight_pointer(&self, height: u64) -> StoragePointer {
        StoragePointer::from_keyword("/staking_weight/").select(&height.to_le_bytes().to_vec())
    }

    fn staking_expire_pointer(&self, height: u64) -> StoragePointer {
        StoragePointer::from_keyword("/staking_expire/").select(&height.to_le_bytes().to_vec())
    }

    fn get_staking_expire(&self, height: u64) -> Decimal {
        let v = self.staking_expire_pointer(height).get();
        if v.len()>0 {
            Staking::descrialize_decimal(&v).unwrap()
        }else{
            Decimal::from(0)
        }
    }

    fn set_staking_expire(&self, height: u64, value: Decimal) {
        self.staking_expire_pointer(height).set(Arc::new(Staking::serialize_decimal(&value).unwrap()));
    }

    fn get_staking_weight(&self, height: u64) -> Decimal { 
        let v = self.staking_weight_pointer(height).get();
        if v.len()>0 {
            return Staking::descrialize_decimal(&v).unwrap();
        }

        let exp = self.get_staking_expire(height);
        let mut w = Decimal::from(0)-exp;
        let mut height = height;
        while height>0 {
            height -= 1;
            let v = self.staking_weight_pointer(height).get();
            if v.len()>0 {
                w +=  Staking::descrialize_decimal(&v).unwrap();
                break;
            }else{
                w -= self.get_staking_expire(height);
            }
        }
        return w;
    }

    fn height_to_no(&self, height: u64) -> u64{
        (height - MINING_FIRST_HEIGHT)/144
    }
    fn set_staking_weight(& self, height: u64, w: Decimal) {
        self.staking_weight_pointer(height).set(Arc::new(Staking::serialize_decimal(&w).unwrap()));
    }


    /// Get the name of the collection
    fn get_name(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        response.data = self.name().into_bytes();

        Ok(response)
    }

    /// Get the symbol of the collection
    fn get_symbol(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        response.data = self.symbol().into_bytes();

        Ok(response)
    }
  

    /// Get the collection identifier
    /// Returns the collection identifier in the format "block:tx"
    fn get_collection_identifier(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Format the collection identifier as "block:tx"
        let identifier = format!("{}:{}", context.myself.block, context.myself.tx);
        response.data = identifier.into_bytes();

        Ok(response)
    }

    /// Get data for a specific orbital
    pub fn get_data(&self, index: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let response = CallResponse::forward(&context.incoming_alkanes);
        //TODO
        Ok(response)
    }

    /// Get attributes for a specific orbital
    pub fn get_attributes(&self, index: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let staking = self.get_staking(index);
        response.data = serde_json::to_vec(&staking)?;
        Ok(response)
    }

    pub fn get_coin_alkanes_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let alkane_id = self.get_coin_id();
        let str_alkane_id = format!("{}:{}", alkane_id.block,alkane_id.tx);
        response.data = str_alkane_id.into_bytes();
        Ok(response)
    }
    
    pub fn get_balance(&self) -> Result<CallResponse> {
        let context = self.context()?; 
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let alkane_id = self.get_coin_id();
        let balance = self.balance(&context.myself, &alkane_id);
        response.data = format!("{}",balance).try_into()?;
        Ok(response)
    }

    pub fn set_storge(&self,key: Vec<u8>,value: Vec<u8>) -> (){
        StoragePointer::wrap(&key).set(Arc::new(value));
    }
}

declare_alkane! {
    impl AlkaneResponder for StakingPool {
        type Message = StakingPoolMessage;
    }
}

#[cfg(test)]
mod test{

    use std::str::RSplit;

    use super::*;
    use hex;
    #[cfg(target_arch = "wasm32")]
    use web_sys::console;
    use wasm_bindgen_test::*;

    macro_rules! test_print {
        ($($arg:tt)*) => {
            #[cfg(target_arch = "wasm32")]
            { console::log_1(&format!($($arg)*).into()) }
            
            #[cfg(not(target_arch = "wasm32"))]
            { println!($($arg)*) }
        };
    }

    #[wasm_bindgen_test]
    fn test_pool(){ 
        let s = StakingPool::default();
        let alkanes_id = AlkaneId::new(2,0x6dc);
        s.set_coin_id(&alkanes_id);
        assert_eq!(s.get_coin_id(),alkanes_id);
    }

    // #[wasm_bindgen_test]
    // fn test_get_profit(){
    //     let sp = StakingPool::default();
    //     let k1 = hex::decode("2f7374616b696e675f7765696768742fc401000000000000").unwrap();
    //     let staking_weight =  Decimal::from_str("20000.0").unwrap();
    //     // let v1 = hex::decode("0732303030302e30").unwrap();
    //     sp.set_storge(k1, Staking::serialize_decimal(&staking_weight).unwrap());

    //     let k1 = hex::decode("2f7374616b696e672f01000000000000000000000000000000").unwrap();
    //     let v1 = hex::decode("00fd00743ba40b000000fb204e1e191c9a279745a8a1f2781984b8b6dd1f2c0a4d65a70504d9fc78032e9fb894d800fbc4010002fba00800").unwrap();
    //     sp.set_storge(k1, v1);

    //     let k1 = hex::decode("2f7374616b696e675f6578706972652fa412000000000000").unwrap();
    //     // let v1 = hex::decode("0732303030302e30").unwrap();
    //     sp.set_storge(k1, Staking::serialize_decimal(&staking_weight).unwrap());

    //     let k1 = hex::decode("2f6f72626974616c5f636f756e74").unwrap();
    //     let v1 = hex::decode("01000000000000000000000000000000").unwrap();
    //     sp.set_storge(k1, v1);

    //     let k1 = hex::decode("2f7374616b696e672f696432696e6465782f02000000000000000000000000000000a0080000000000000000000000000000").unwrap();
    //     let v1 = hex::decode("01000000000000000000000000000000").unwrap();
    //     sp.set_storge(k1, v1);
    //     let (p,r,w) = sp.calc_profit(1, 457).unwrap();
        
    //     assert_eq!(p,5015432098765);
    //     assert_eq!(r,386993217);
    //     assert_eq!(w,0);
    //     let (p1,r1,w1) =sp.calc_profit_1(1, 457).unwrap();
    //     test_print!("{:?} {:?} {:?}",p1,r1,w1);
    //     assert_eq!(p,p1);
    //     assert_eq!(r,r1);
    //     assert_eq!(w,w1);

    // }

    #[wasm_bindgen_test]
    fn test_get_profit2(){
        let sp = StakingPool::default();
        let index = sp.get_brc20_count() +1;
        let staking = Staking { brc20_index: 0,
            brc20_value: 800000000,
            staking_value: 50000, period: 60,
            tx: [0;32],
            invite_index: 0,
            staking_height: 455,
            unstaking_height: 0,
            alkanes_id: [2,111128],
            withdraw_coin_value: 0 };

        sp.add_staking(index as u128, &staking);

        let (p,r,w) = sp.calc_profit(index as u128, 468).unwrap();
        let (p1,r1,w1) =sp.calc_profit_1(index as u128, 468).unwrap();
        test_print!("{:?} {:?} {:?}",p1,r1,w1);
        assert_eq!(p,p1);
        assert_eq!(r,r1);
        assert_eq!(w,w1);

        let (p,r,w) = sp.calc_profit(index as u128, 599).unwrap();
        let (p1,r1,w1) =sp.calc_profit_1(index as u128, 599).unwrap();
        test_print!("{:?} {:?} {:?}",p1,r1,w1);
        assert_eq!(p,p1);
        assert_eq!(r,r1);
        assert_eq!(w,w1);

        let (p,r,w) = sp.calc_profit(index as u128, 650).unwrap();
        let (p1,r1,w1) =sp.calc_profit_1(index as u128, 650).unwrap();
        test_print!("{:?} {:?} {:?}",p1,r1,w1);
        assert_eq!(p,p1);
        assert_eq!(r,r1);
        assert_eq!(w,w1);

        let (p,r,w) = sp.calc_profit(index as u128, 750).unwrap();
        let (p1,r1,w1) =sp.calc_profit_1(index as u128, 750).unwrap();
        test_print!("{:?} {:?} {:?}",p1,r1,w1);
        assert_eq!(p,p1);
        assert_eq!(r,r1);
        assert_eq!(w,w1);

    }
}