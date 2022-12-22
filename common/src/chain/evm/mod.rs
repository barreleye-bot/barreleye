use async_trait::async_trait;
use ethers::{
	self,
	abi::AbiDecode,
	prelude::*,
	types::{Address, Log, Transaction, TransactionReceipt, U256, U64},
	utils::hex::ToHex,
};
use eyre::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
	cache::CacheKey,
	chain::{ChainTrait, ModuleId, ModuleTrait, WarehouseData},
	models::Network,
	utils, BlockHeight, Cache, RateLimiter,
};
use modules::{EvmBalance, EvmModuleTrait, EvmTokenBalance, EvmTokenTransfer, EvmTransfer};

mod modules;

static TRANSFER_FROM_TO_AMOUNT: &str =
	"ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[derive(Debug, Eq, PartialEq)]
pub enum EvmTopic {
	Unknown,
	TokenTransfer(Address, Address, U256),
}

pub struct Evm {
	_cache: Arc<RwLock<Cache>>,
	network: Network,
	rpc: Option<String>,
	provider: Option<Arc<Provider<RetryClient<Http>>>>,
	rate_limiter: Option<Arc<RateLimiter>>,
	modules: Vec<Box<dyn EvmModuleTrait>>,
}

impl Evm {
	pub fn new(cache: Arc<RwLock<Cache>>, network: Network) -> Self {
		let rps = network.rps as u32;
		let network_id = network.network_id;

		Self {
			_cache: cache,
			network,
			rpc: None,
			provider: None,
			rate_limiter: utils::get_rate_limiter(rps),
			modules: vec![
				Box::new(EvmTransfer::new(network_id)),
				Box::new(EvmBalance::new(network_id)),
				Box::new(EvmTokenTransfer::new(network_id)),
				Box::new(EvmTokenBalance::new(network_id)),
			],
		}
	}
}

#[async_trait]
impl ChainTrait for Evm {
	async fn connect(&mut self) -> Result<bool> {
		let rpc_endpoints: Vec<String> =
			serde_json::from_value(self.network.rpc_endpoints.clone())?;

		for url in rpc_endpoints.into_iter() {
			if let Ok(provider) = Provider::<RetryClient<Http>>::new_client(&url, 10, 1_000) {
				if let Some(rate_limiter) = &self.rate_limiter {
					rate_limiter.until_ready().await;
				}

				if provider.get_block_number().await.is_ok() {
					self.rpc = Some(url);
					self.provider = Some(Arc::new(provider));

					break;
				}
			}
		}

		Ok(self.is_connected())
	}

	fn is_connected(&self) -> bool {
		self.provider.is_some()
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
		if address.len() > 2 {
			if let Ok(parsed_address) = address[2..].parse() {
				return ethers::utils::to_checksum(&parsed_address, None);
			}
		}

		address.to_string()
	}

	async fn get_block_height(&self) -> Result<BlockHeight> {
		self.rate_limit().await;
		Ok(self.provider.as_ref().unwrap().get_block_number().await?.as_u64())
	}

	async fn process_block(
		&self,
		block_height: BlockHeight,
		module_ids: Vec<ModuleId>,
	) -> Result<Option<WarehouseData>> {
		let mut ret = None;
		let provider = self.provider.as_ref().unwrap();

		self.rate_limit().await;
		match provider.get_block_with_txs(block_height).await? {
			Some(block) if block.number.is_some() => {
				let mut warehouse_data = WarehouseData::new();

				for tx in block.transactions.into_iter() {
					// skip if pending
					if tx.block_hash.is_none() {
						continue;
					}

					// process tx only if receipt exists
					self.rate_limit().await;
					if let Some(receipt) = provider.get_transaction_receipt(tx.hash()).await? {
						// skip if tx reverted
						if let Some(status) = receipt.status {
							if status == U64::zero() {
								continue;
							}
						}

						// process tx
						warehouse_data += self
							.process_transaction(
								block_height,
								block.timestamp.as_u32(),
								tx,
								receipt,
								module_ids.clone(),
							)
							.await?;
					}
				}

				ret = Some(warehouse_data);
			}
			_ => {}
		}

		Ok(ret)
	}
}

impl Evm {
	async fn process_transaction(
		&self,
		block_height: BlockHeight,
		block_time: u32,
		tx: Transaction,
		receipt: TransactionReceipt,
		module_ids: Vec<ModuleId>,
	) -> Result<WarehouseData> {
		let mut ret = WarehouseData::new();

		for module in self.modules.iter().filter(|m| module_ids.contains(&m.get_id())) {
			ret += module.run(self, block_height, block_time, tx.clone(), receipt.clone()).await?;
		}

		Ok(ret)
	}

	async fn _is_smart_contract(&self, address: &H160) -> Result<bool> {
		let cache_key = CacheKey::EvmSmartContract(
			self.network.network_id as u64,
			ethers::utils::to_checksum(address, None),
		);

		Ok(match self._cache.read().await.get::<bool>(cache_key.clone()).await? {
			Some(v) => v,
			_ => {
				self.rate_limit().await;
				let is_smart_contract =
					!self.provider.as_ref().unwrap().get_code(*address, None).await?.is_empty();
				self._cache.read().await.set::<bool>(cache_key, is_smart_contract).await?;
				is_smart_contract
			}
		})
	}

	fn get_topic(&self, log: &Log) -> Result<EvmTopic> {
		if log.topics.len() == 3 && log.topics[0].encode_hex::<String>() == *TRANSFER_FROM_TO_AMOUNT
		{
			let from = Address::from(log.topics[1]);
			let to = Address::from(log.topics[2]);
			let amount = U256::decode(log.data.clone()).unwrap_or_default();

			return Ok(EvmTopic::TokenTransfer(from, to, amount));
		}

		Ok(EvmTopic::Unknown)
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
		let network = Network { blockchain: Blockchain::Evm, ..Default::default() };
		let evm = Evm::new(cache, network);

		assert_eq!(evm.format_address(""), "");
		assert_eq!(
			evm.format_address("0xd8da6bf26964af9d7eed9e03e53415d37aa96045"),
			"0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"
		);
	}
}
