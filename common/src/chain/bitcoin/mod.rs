use async_trait::async_trait;
use bitcoin::{
	blockdata::transaction::Transaction, hash_types::Txid, util::address::Address,
	Network as BitcoinNetwork,
};
use eyre::Result;
use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::RwLock;
use url::Url;

use crate::{
	cache::CacheKey,
	chain::{ChainTrait, ModuleId, ModuleTrait, WarehouseData},
	models::Network,
	utils, BlockHeight, Cache, RateLimiter,
};
use client::{Auth, Client};
use modules::{BitcoinBalance, BitcoinCoinbase, BitcoinLink, BitcoinModuleTrait, BitcoinTransfer};

mod client;
mod modules;

pub struct Bitcoin {
	cache: Arc<RwLock<Cache>>,
	network: Network,
	rpc: Option<String>,
	client: Option<Arc<Client>>,
	bitcoin_network: BitcoinNetwork,
	rate_limiter: Option<Arc<RateLimiter>>,
	modules: Vec<Box<dyn BitcoinModuleTrait>>,
}

impl Bitcoin {
	pub fn new(cache: Arc<RwLock<Cache>>, network: Network) -> Self {
		let chain_id = network.chain_id as u32;
		let rps = network.rps as u32;
		let network_id = network.network_id;

		Self {
			cache,
			network,
			rpc: None,
			client: None,
			bitcoin_network: BitcoinNetwork::from_magic(chain_id)
				.unwrap_or(BitcoinNetwork::Bitcoin),
			rate_limiter: utils::get_rate_limiter(rps),
			modules: vec![
				Box::new(BitcoinTransfer::new(network_id)),
				Box::new(BitcoinBalance::new(network_id)),
				Box::new(BitcoinLink::new(network_id)),
				Box::new(BitcoinCoinbase::new(network_id)),
			],
		}
	}
}

#[async_trait]
impl ChainTrait for Bitcoin {
	async fn connect(&mut self) -> Result<bool> {
		let rpc_endpoints: Vec<String> =
			serde_json::from_value(self.network.rpc_endpoints.clone())?;

		for url in rpc_endpoints.into_iter() {
			if let Ok(u) = Url::parse(&url) {
				let auth = match (u.username(), u.password()) {
					(username, Some(password)) => {
						Auth::UserPass(username.to_string(), password.to_string())
					}
					_ => Auth::None,
				};

				if let Some(rate_limiter) = &self.rate_limiter {
					rate_limiter.until_ready().await;
				}

				let client = Client::new_without_retry(&url, auth.clone());
				if client.get_blockchain_info().await.is_ok() {
					self.client = Some(Arc::new(Client::new(&url, auth)));
					self.rpc = Some(url);

					break;
				}
			}
		}

		Ok(self.is_connected())
	}

	fn is_connected(&self) -> bool {
		self.client.is_some()
	}

	fn get_network(&self) -> Network {
		self.network.clone()
	}

	fn get_rpc(&self) -> Option<String> {
		self.rpc.clone()
	}

	fn get_module_ids(&self) -> Vec<ModuleId> {
		self.modules.iter().map(|m| m.get_id()).collect()
	}

	fn get_rate_limiter(&self) -> Option<Arc<RateLimiter>> {
		self.rate_limiter.clone()
	}

	fn format_address(&self, address: &str) -> String {
		match Address::from_str(address) {
			Ok(parsed_address) => parsed_address.to_string(),
			_ => address.to_string(),
		}
	}

	async fn get_block_height(&self) -> Result<BlockHeight> {
		self.rate_limit().await;
		Ok(self.client.as_ref().unwrap().get_block_count().await?)
	}

	async fn process_block(
		&self,
		block_height: BlockHeight,
		module_ids: Vec<ModuleId>,
	) -> Result<Option<WarehouseData>> {
		let mut ret = None;

		self.rate_limit().await;
		if let Ok(block_hash) = self.client.as_ref().unwrap().get_block_hash(block_height).await {
			self.rate_limit().await;
			if let Ok(block) = self.client.as_ref().unwrap().get_block(&block_hash).await {
				let mut warehouse_data = WarehouseData::new();

				for tx in block.txdata.into_iter() {
					warehouse_data += self
						.process_transaction(
							block_height,
							block.header.time,
							tx,
							module_ids.clone(),
						)
						.await?;
				}

				ret = Some(warehouse_data);
			}
		}

		Ok(ret)
	}
}

impl Bitcoin {
	async fn process_transaction(
		&self,
		block_height: BlockHeight,
		block_time: u32,
		tx: Transaction,
		module_ids: Vec<ModuleId>,
	) -> Result<WarehouseData> {
		let mut ret = WarehouseData::new();

		let get_unique_addresses = move |pair: Vec<(String, u64)>| {
			let mut m = HashMap::<String, u64>::new();

			for p in pair.into_iter() {
				let (address, value) = p;
				let address_key = address.to_string();

				let initial_value = m.get(&address_key).unwrap_or(&0);
				m.insert(address_key, *initial_value + value);
			}

			m
		};

		let inputs = get_unique_addresses({
			let mut ret = vec![];

			for txin in tx.input.iter() {
				let (txid, vout) = (txin.previous_output.txid, txin.previous_output.vout);

				if !txid.is_empty() && !tx.is_coin_base() {
					if let Some((a, v)) = self.get_utxo(txid, vout).await? {
						ret.push((a, v))
					}
				}
			}

			ret
		});

		let outputs =
			get_unique_addresses(self.index_transaction_outputs(block_height, &tx).await?);

		for module in self.modules.iter().filter(|m| module_ids.contains(&m.get_id())) {
			ret += module
				.run(self, block_height, block_time, tx.clone(), inputs.clone(), outputs.clone())
				.await?;
		}

		Ok(ret)
	}

	async fn index_transaction_outputs(
		&self,
		block_height: BlockHeight,
		tx: &Transaction,
	) -> Result<Vec<(String, u64)>> {
		let mut ret = vec![];

		for (i, txout) in tx.output.iter().enumerate() {
			if let Some(address) = self.get_address(tx, i as u32)? {
				let cache_key = CacheKey::BitcoinTxIndex(
					self.network.network_id as u64,
					tx.txid().as_hash().to_string(),
				);

				self.cache.read().await.set::<u64>(cache_key, block_height).await?;

				ret.push((address, txout.value));
			}
		}

		Ok(ret)
	}

	async fn get_utxo(&self, txid: Txid, vout: u32) -> Result<Option<(String, u64)>> {
		let cache_key =
			CacheKey::BitcoinTxIndex(self.network.network_id as u64, txid.as_hash().to_string());

		let mut block_hash = None;
		if let Some(block_height) = self.cache.read().await.get::<u64>(cache_key.clone()).await? {
			self.rate_limit().await;
			block_hash = Some(self.client.as_ref().unwrap().get_block_hash(block_height).await?);
			// @NOTE do not delete the "used up" utxo here; modules are stateless and another one
			// might need to use it
		}

		// `block_hash` will always be *some value* for those modules that have
		// started indexing from block 1; for all others -txindex is needed
		self.rate_limit().await;
		let tx =
			self.client.as_ref().unwrap().get_raw_transaction(&txid, block_hash.as_ref()).await?;
		let ret = self.get_address(&tx, vout)?.map(|a| {
			let v = tx.output[vout as usize].value;
			(a, v)
		});

		Ok(ret)
	}

	fn get_address(&self, tx: &Transaction, vout: u32) -> Result<Option<String>> {
		let mut ret = None;

		if vout < tx.output.len() as u32 {
			if let Ok(address) =
				Address::from_script(&tx.output[vout as usize].script_pubkey, self.bitcoin_network)
			{
				ret = Some(address.to_string());
			} else {
				ret = Some(format!("{}:{}", tx.txid().as_hash(), vout));
			}
		}

		Ok(ret)
	}

	fn is_valid_address(&self, address: &str) -> bool {
		!address.contains(':')
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Blockchain, Settings};
	use futures::executor::block_on;

	#[test]
	fn test_format_address() {
		let settings = Settings::new().unwrap();
		let cache = Arc::new(RwLock::new(block_on(Cache::new(Arc::new(settings))).unwrap()));
		let network = Network { blockchain: Blockchain::Bitcoin, ..Default::default() };
		let bitcoin = Bitcoin::new(cache, network);

		assert_eq!(bitcoin.format_address(""), "");
		assert_eq!(
			bitcoin.format_address("12iAWCJdrX2n3A9q1XzpfFHDUeNGMSWWcR"),
			"12iAWCJdrX2n3A9q1XzpfFHDUeNGMSWWcR"
		);
	}
}
