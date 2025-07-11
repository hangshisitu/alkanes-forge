
use alkanes_support::id::AlkaneId;
use alkanes_support::witness::find_witness_payload;
use metashrew_support::utils::{consume_exact, consume_sized_int, consume_to_end,consensus_decode};
use anyhow::{anyhow, Ok, Result};
use bincode::{config, serde::decode_from_slice, serde::encode_to_vec};
use bitcoin::Transaction;
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
use std::io::Cursor;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]

//所有区间采用前闭后开
//
pub struct Staking {
    pub brc20_index: u8,
    pub brc20_value: u128,
    pub staking_value: u128,
    pub period: u16,
    pub tx: [u8;32],
    pub invite_index: u128,
    pub staking_height: u64,      //质押brc20转账交易所在区块高度，开始产生收益, +1开始释放
    pub unstaking_height: u64,    //解质押交易所在区块高度, 该高度不算收益
    // pub expire_height: u64,       //过期区块高度，该高度不算收益  staking_height + period * 144
    pub alkanes_id: [u128;2],
    pub withdraw_coin_value: u128,
}

impl Staking {

    pub fn from_tx( raw_tx: Vec<u8>) -> Result<Self> {
        let tx = consensus_decode::<Transaction>(&mut Cursor::new(raw_tx))?;
        let data: Vec<u8> = find_witness_payload(&tx, 0).unwrap_or_else(|| vec![]);
        Staking::from_vec8(data)
    }

    pub fn from_vec8(data: Vec<u8>) -> Result<Self> {
        let mut cursor = Cursor::<Vec<u8>>::new(data);
        Ok(Staking {
            brc20_index: consume_sized_int::<u8>(&mut cursor)?,
            brc20_value: consume_sized_int::<u128>(&mut cursor)?,
            staking_value: consume_sized_int::<u128>(&mut cursor)?,
            period:  consume_sized_int::<u16>(&mut cursor)?,
            tx: consume_exact(&mut cursor,32)?.try_into().unwrap(),
            alkanes_id: [consume_sized_int::<u128>(&mut cursor)?,consume_sized_int::<u128>(&mut cursor)?],
            staking_height: consume_sized_int::<u64>(&mut cursor)?,
            invite_index: 0,
            unstaking_height: 0,
            withdraw_coin_value: 0,
        })
    }

    pub fn get_expire_height(&self) -> u64 {
        self.staking_height + self.period as u64 * 144
    }

    pub fn get_alanes_id(&self) -> AlkaneId {
        AlkaneId { block: self.alkanes_id[0], tx: self.alkanes_id[1] }
    }
    pub fn get_mining_end_height(&self,height:u64) -> u64 {
        if self.unstaking_height>0{
            min(min(self.unstaking_height,self.get_expire_height()),height as u64)
        }else{
            min(self.get_expire_height(),height as u64)
        }
    }

    pub fn get_release_end_height(&self,height:u64)-> u64{
        if self.unstaking_height>0{
            min(self.unstaking_height,height as u64)
        }else{
            height as u64
        }
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        encode_to_vec(self, config::standard()).map_err(|e| anyhow!("serialize error:{}", e))
    }


    pub fn descrialize(v: &Vec<u8>) -> Result<Self> {
        let (staking,_) = decode_from_slice(v,config::standard()).map_err(|e|anyhow!("descrialize error:{}", e))?;
        Ok(staking)
    }

    pub fn serialize_invite_vec(v: &Vec<u128>) -> Result<Vec<u8>>{
        encode_to_vec(v, config::standard()).map_err(|e| anyhow!("serialize error:{}", e))
    }

    pub fn descrialize_invite_vec(v: &Vec<u8>) -> Result<Vec<u128>>{
        let (invite_vec,_) = decode_from_slice(v,config::standard()).map_err(|e|anyhow!("descrialize error:{}", e))?;
        Ok(invite_vec)
    }

    pub fn serialize_decimal(d: &Decimal) -> Result<Vec<u8>>{
        encode_to_vec(d, config::standard()).map_err(|e| anyhow!("serialize error:{}", e))
    }

    pub fn descrialize_decimal(v: &Vec<u8>) -> Result<Decimal>{
        let (decimal,_) = decode_from_slice(v,config::standard()).map_err(|e|anyhow!("descrialize error:{}", e))?;
        Ok(decimal)
    }

}


#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]
pub struct StakingStat {
    #[serde(with = "rust_decimal::serde::str")]
    pub staking_weight: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub unstaking_weight: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub expire_weight: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub init_weight: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub weight: Decimal,
}

impl StakingStat {

    pub fn total_weight(&self) -> Decimal {
        self.init_weight + self.staking_weight - self.expire_weight - self.unstaking_weight
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        encode_to_vec(self, config::standard()).map_err(|e| anyhow!("serialize error:{}", e))
    }


    pub fn descrialize(v: &Vec<u8>) -> Result<Self> {
        let (staking_stat,_) = decode_from_slice(v,config::standard()).map_err(|e|anyhow!("descrialize error:{}", e))?;
        Ok(staking_stat)
    }
}

#[cfg(test)]
mod test{

    use super::*;
    use hex;
    #[cfg(target_arch = "wasm32")]
    use web_sys::console;
    use wasm_bindgen_test::*;
    use serde_json;

    macro_rules! test_print {
        ($($arg:tt)*) => {
            #[cfg(target_arch = "wasm32")]
            { console::log_1(&format!($($arg)*).into()) }
            
            #[cfg(not(target_arch = "wasm32"))]
            { println!($($arg)*) }
        };
    }

    #[wasm_bindgen_test]
    fn test_staking(){ 
        let s = Staking::default();
        let v = s.serialize().unwrap();
        // test_print!("{}",hex::encode(&v.clone()));
        let s2 = Staking::descrialize(&v.clone()).unwrap();
        assert_eq!(s,s2);

        let ss = Staking{
            brc20_index:1,
            brc20_value: 100000,
            staking_value: 100000,
            period: 30,
            tx: [7;32],
            invite_index: 0,
            staking_height: 8459900,
            unstaking_height: 0,
            alkanes_id: [2,12890],
            withdraw_coin_value: 893400,
        };
        let vv = ss.serialize().unwrap();
        // test_print!("{}",hex::encode(&vv.clone()));
        assert_eq!(ss,Staking::descrialize(&vv).unwrap());
    }

    #[wasm_bindgen_test]
    fn test_invite_vec(){
        let inv = [23u128,10];
        let s = Staking::serialize_invite_vec(&inv.to_vec()).unwrap();
        // test_print!("invite_vec {}",hex::encode(&s.clone()));
        assert_eq!(inv.to_vec(),Staking::descrialize_invite_vec(&s).unwrap())
    }

    #[wasm_bindgen_test]
    fn test_staking_stat(){ 
        let s = StakingStat::default();
        let v = s.serialize().unwrap();
        // test_print!("stakingstat {}",hex::encode(&v.clone()));
        let s2 = StakingStat::descrialize(&v.clone()).unwrap();
        assert_eq!(s,s2);

        let ss = StakingStat{
            staking_weight: Decimal::from_str("100000.33").unwrap(),
            unstaking_weight: Decimal::from_str("100000.335678").unwrap(),
            expire_weight: Decimal::from_str("100000.23444433").unwrap(),
            init_weight: Decimal::from_str("100000.23444433").unwrap(),
            weight: Decimal::from_str("100000.23444433").unwrap(),
        };
        let vv = ss.serialize().unwrap();
        let ss2 = StakingStat::descrialize(&vv).unwrap();
        assert_eq!(ss,ss2);
    }

    #[wasm_bindgen_test]
    fn test_json(){
        let (p,r,w) = (1u128,10u128,100u128);
        let s = serde_json::to_string(&(p,r,w)).unwrap();
        test_print!("json {}",s);
        test_print!("json 2 {}",String::from_utf8(serde_json::to_vec(&(p,r,w)).unwrap()).unwrap());
    }

    #[wasm_bindgen_test]
    fn test_from(){
        let s = "01005039278c0400000000000000000000307500000000000000000000000000006801191c9a279745a8a1f2781984b8b6dd1f2c0a4d65a70504d9fc78032e9fb894d8000000000000000000000000000000000000000000000000000000000000000088c50d0000000000";
        let v = hex::decode(s).unwrap();
        let staking = Staking::from_vec8(v).unwrap();
        assert_eq!(staking, Staking{
            brc20_index:1,
            brc20_value:5000000000000,
            staking_value:30000,
            period:360,
            tx: hex::decode("191c9a279745a8a1f2781984b8b6dd1f2c0a4d65a70504d9fc78032e9fb894d8").unwrap().try_into().unwrap(),
            invite_index:0,
            staking_height:902536,
            unstaking_height:0,
            alkanes_id: [0, 0],
            withdraw_coin_value:0,
        });
    }

    #[wasm_bindgen_test]
    fn test_from_tx(){
        let raw_tx = hex::decode("02000000000101b3b1f7252af64d70c00da99725a383d5ef3826072e3b61cc9b117209226b096d0000000000ffffffff0222020000000000002251207ca00ebfa26de5057dbdd3f26856cdd9722a9b7851e097a4c665f95f2aae500100000000000000000e6a5d0bff7f818cec82d08bc0a832034035abb02620b67a034a9a91ad741cb59fd0f54dbd9c674b5b977aea9f5d1b405637ece05698f66c09018ea9a432bd9fb447ed3d65d16692932058dfff8f10ae04972078bc362031e719bee54b3359292770e35f0adcce3970a749683ec9f9bb029ab3ac00630342494e004c6b0000743ba40b0000000000000000000000204e00000000000000000000000000001e00191c9a279745a8a1f2781984b8b6dd1f2c0a4d65a70504d9fc78032e9fb894d80000000000000000000000000000000000000000000000000000000000000000bf010000000000006821c178bc362031e719bee54b3359292770e35f0adcce3970a749683ec9f9bb029ab300000000").unwrap();
        let ret = Staking::from_tx(raw_tx);
        assert_eq!(ret.is_ok(), true);
        if ret.is_ok() {
            test_print!("tx staking {:?}",ret.unwrap());
        }
        
    }
}