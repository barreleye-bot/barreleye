use console::style;
use derive_more::Display;
use eyre::Result;
use num_format::{SystemLocale, ToFormattedString};
use sea_orm::ColumnTrait;
use serde_json::Value as JsonValue;
use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
	time::SystemTime,
};
use tokio::{
	signal,
	sync::{
		broadcast,
		mpsc::{Receiver, Sender},
		watch,
	},
	task::JoinSet,
	time::{sleep, Duration},
};
use uuid::Uuid;

use barreleye_common::{
	chain::{ModuleId, WarehouseData},
	models::{
		Address, AddressColumn, Amount, Balance, Config, ConfigKey, Entity, Link, Network,
		NetworkColumn, PrimaryId, PrimaryIds, Relation, SoftDeleteModel, Transfer,
	},
	utils, App, AppError, BlockHeight, Progress, ProgressReadyType, ProgressStep, Verbosity,
	Warnings, INDEXER_HEARTBEAT,
};

mod blocks;
mod upstream;

#[derive(Clone, Debug)]
struct NetworkParams {
	pub network_id: PrimaryId,
	pub range: (BlockHeight, Option<BlockHeight>),
	pub modules: Vec<ModuleId>,
}

impl NetworkParams {
	pub fn new(
		network_id: PrimaryId,
		min: BlockHeight,
		max: Option<BlockHeight>,
		modules: &[ModuleId],
	) -> Self {
		Self { network_id, range: (min, max), modules: modules.to_vec() }
	}
}

#[derive(Display, Debug)]
enum IndexType {
	#[display(fmt = "blocks")]
	Blocks,
	#[display(fmt = "upstream")]
	Upstream,
}

#[derive(Clone)]
pub struct Indexer {
	app: Arc<App>,
}

impl Indexer {
	pub fn new(app: Arc<App>) -> Self {
		Self { app }
	}

	pub async fn start(&self, warnings: Warnings, progress: Progress) -> Result<()> {
		if self.app.settings.is_indexer && !self.app.settings.is_server {
			progress.show(ProgressStep::Ready(ProgressReadyType::Indexer, warnings));
		}

		loop {
			self.prune_data().await?;

			let mut set = JoinSet::new();
			let (tx, rx) = watch::channel(SystemTime::now());

			set.spawn({
				let s = self.clone();
				let r = rx.clone();
				async move { s.index_blocks(r).await }
			});

			set.spawn({
				let s = self.clone();
				let r = rx.clone();
				async move { s.index_upstream(r).await }
			});

			let ret = tokio::select! {
				_ = signal::ctrl_c() => Ok(()),
				v = self.primary_check() => v,
				v = self.networks_check(tx) => v,
				v = async {
					while let Some(res) = set.join_next().await {
						let _ = res?;
					}

					Ok(())
				} => v,
			};

			if let Err(err) = ret {
				return Err(AppError::Indexing { error: err.to_string() }.into());
			}
		}
	}

	async fn primary_check(&self) -> Result<()> {
		let indexer_promotion = self.app.settings.indexer_promotion;
		let db = self.app.db();
		let uuid = self.app.uuid;

		loop {
			let cool_down_period = utils::ago_in_seconds(indexer_promotion / 2);

			let last_primary = Config::get::<_, Uuid>(db, ConfigKey::Primary).await?;
			match last_primary {
				None => {
					// first run ever
					Config::set::<_, Uuid>(db, ConfigKey::Primary, uuid).await?;
				}
				Some(hit) if hit.value == uuid && hit.updated_at >= cool_down_period => {
					// if primary, check-in only if cool-down period has not started yet ↑
					if Config::set_where::<_, Uuid>(db, ConfigKey::Primary, uuid, hit).await? {
						self.app.set_is_primary(true).await?;
					}
				}
				Some(hit) if utils::ago_in_seconds(indexer_promotion) > hit.updated_at => {
					// attempt to upgrade to primary (set is_primary on the next iteration)
					Config::set_where::<_, Uuid>(db, ConfigKey::Primary, uuid, hit).await?;
				}
				_ => {
					// either cool-down period has started or this is a secondary
					self.app.set_is_primary(false).await?;
				}
			}

			sleep(Duration::from_secs(INDEXER_HEARTBEAT)).await
		}
	}

	async fn networks_check(&self, tx: watch::Sender<SystemTime>) -> Result<()> {
		let mut networks_updated_at =
			Config::get::<_, u8>(self.app.db(), ConfigKey::NetworksUpdated)
				.await?
				.map(|v| v.updated_at)
				.unwrap_or_else(utils::now);

		loop {
			match Config::get::<_, u8>(self.app.db(), ConfigKey::NetworksUpdated).await? {
				Some(value) if value.updated_at != networks_updated_at => {
					networks_updated_at = value.updated_at;
					tx.send(SystemTime::now())?;
				}
				_ => {}
			}

			sleep(Duration::from_secs(1)).await;
		}
	}

	async fn prune_data(&self) -> Result<()> {
		// prune all soft-deleted addresses
		let addresses = Address::get_all_deleted(self.app.db()).await?;
		if !addresses.is_empty() {
			// delete all upstream configs
			Config::delete_many(
				self.app.db(),
				addresses
					.iter()
					.map(|a| ConfigKey::IndexerUpstreamSync(a.network_id, a.address_id))
					.collect(),
			)
			.await?;

			// delete all addresses
			Address::prune_all_where(
				self.app.db(),
				AddressColumn::AddressId.is_in(Into::<PrimaryIds>::into(addresses.clone())),
			)
			.await?;

			// delete links from warehouse
			let mut sources: HashMap<PrimaryId, HashSet<String>> = HashMap::new();
			for address in addresses.into_iter() {
				if let Some(set) = sources.get_mut(&address.network_id) {
					set.insert(address.address);
				} else {
					sources.insert(address.network_id, HashSet::from([address.address]));
				}
			}
			Link::delete_all_by_sources(&self.app.warehouse, sources).await?;
		}

		// prune all soft-deleted entities
		Entity::prune_all(self.app.db()).await?;

		// prune all soft-deleted networks
		let deleted_networks = Network::get_all_deleted(self.app.db()).await?;
		if !deleted_networks.is_empty() {
			let network_ids: PrimaryIds = deleted_networks.clone().into();

			// delete all associated configs
			Config::delete_all_by_keywords(
				self.app.db(),
				deleted_networks.clone().iter().map(|n| format!("n{}", n.network_id)).collect(),
			)
			.await?;

			// delete all addresses
			Address::prune_all_where(
				self.app.db(),
				AddressColumn::NetworkId.is_in(network_ids.clone()),
			)
			.await?;

			// delete from warehouse
			let (
				transfers_deleted,
				relations_deleted,
				balances_deleted,
				amounts_deleted,
				links_deleted,
			) = tokio::join!(
				Transfer::delete_all_by_network_id(&self.app.warehouse, network_ids.clone()),
				Relation::delete_all_by_network_id(&self.app.warehouse, network_ids.clone()),
				Balance::delete_all_by_network_id(&self.app.warehouse, network_ids.clone()),
				Amount::delete_all_by_network_id(&self.app.warehouse, network_ids.clone()),
				Link::delete_all_by_network_id(&self.app.warehouse, network_ids.clone()),
			);

			transfers_deleted
				.and(relations_deleted)
				.and(balances_deleted)
				.and(amounts_deleted)
				.and(links_deleted)?;

			// finally delete only the networks we grabbed earlier
			Network::prune_all_where(self.app.db(), NetworkColumn::NetworkId.is_in(network_ids))
				.await?;
		}

		Ok(())
	}

	fn log(&self, index_type: IndexType, detailed: bool, message: &str) {
		if self.app.settings.verbosity > Verbosity::Silent || !detailed {
			println!(
				"{} {}: {message}",
				style("Indexer").cyan().bold(),
				style(format!("({index_type})")).dim()
			);
		}
	}

	fn format_number(&self, n: usize) -> Result<String> {
		let locale = SystemLocale::default()?;
		Ok(n.to_formatted_string(&locale))
	}
}

pub struct Pipe {
	config_key: ConfigKey,
	sender: Sender<(ConfigKey, JsonValue, WarehouseData)>,
	receipt: Receiver<()>,
	pub abort: broadcast::Receiver<()>,
}

impl Pipe {
	pub fn new(
		config_key: ConfigKey,
		sender: Sender<(ConfigKey, JsonValue, WarehouseData)>,
		receipt: Receiver<()>,
		abort: broadcast::Receiver<()>,
	) -> Self {
		Self { config_key, sender, receipt, abort }
	}

	pub async fn push(
		&mut self,
		config_value: JsonValue,
		warehouse_data: WarehouseData,
	) -> Result<()> {
		self.sender.send((self.config_key, config_value, warehouse_data)).await?;

		tokio::select! {
			_ = self.receipt.recv() => {}
			_ = self.abort.recv() => {}
		}

		Ok(())
	}
}
