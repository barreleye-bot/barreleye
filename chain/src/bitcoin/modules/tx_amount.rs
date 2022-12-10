use async_trait::async_trait;
use bitcoin::blockdata::transaction::Transaction;
use eyre::Result;
use primitive_types::U256;
use std::collections::HashMap;

use crate::{bitcoin::modules::BitcoinModuleTrait, Bitcoin, ModuleTrait, WarehouseData};
use barreleye_common::{
	models::{PrimaryId, TxAmount},
	BlockHeight, ChainModuleId,
};

pub struct BitcoinTxAmount {
	network_id: PrimaryId,
}

impl ModuleTrait for BitcoinTxAmount {
	fn new(network_id: PrimaryId) -> Self
	where
		Self: Sized,
	{
		Self { network_id }
	}

	fn get_id(&self) -> ChainModuleId {
		ChainModuleId::BitcoinTxAmount
	}
}

#[async_trait]
impl BitcoinModuleTrait for BitcoinTxAmount {
	async fn run(
		&self,
		_bitcoin: &Bitcoin,
		block_height: BlockHeight,
		block_time: u32,
		tx: Transaction,
		inputs: HashMap<String, u64>,
		outputs: HashMap<String, u64>,
	) -> Result<WarehouseData> {
		let mut ret = WarehouseData::new();
		let mut tx_amount_map = HashMap::<String, (u64, u64)>::new();

		if !tx.is_coin_base() {
			for (address, new_amount) in inputs.into_iter() {
				if let Some(amounts) = tx_amount_map.get_mut(&address) {
					amounts.1 += new_amount;
				} else {
					tx_amount_map.insert(address, (0, new_amount));
				}
			}
		}

		for (address, new_amount) in outputs.into_iter() {
			if let Some(amounts) = tx_amount_map.get_mut(&address) {
				amounts.0 += new_amount;
			} else {
				tx_amount_map.insert(address, (new_amount, 0));
			}
		}

		let tx_hash = tx.txid().as_hash().to_string();

		for (address, (amount_in, amount_out)) in tx_amount_map.into_iter() {
			ret.tx_amounts.insert(TxAmount::new(
				self.get_id(),
				self.network_id,
				block_height,
				tx_hash.clone(),
				address.into(),
				None,
				U256::from_str_radix(&amount_in.to_string(), 10)?,
				U256::from_str_radix(&amount_out.to_string(), 10)?,
				block_time,
			));
		}

		Ok(ret)
	}
}
