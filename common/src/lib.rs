use chrono::NaiveDateTime;
use clap::{builder::PossibleValue, ValueEnum};
use console::{style, Emoji};
use derive_more::Display;
use eyre::{bail, eyre, Result};
use futures::future::join_all;
use governor::{
	clock::DefaultClock,
	state::{direct::NotKeyed, InMemoryState},
	RateLimiter as GovernorRateLimiter,
};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use itertools::{Either, Itertools};
use sea_orm::{entity::prelude::*, DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};
use std::{
	collections::HashMap,
	process,
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
};
use tokio::{sync::RwLock, time::Duration};

use crate::{
	chain::{Bitcoin, BoxedChain, Evm},
	models::{Config, ConfigKey, Network, PrimaryId},
};
pub use cache::Cache;
pub use db::Db;
pub use errors::AppError;
pub use progress::{Progress, ReadyType as ProgressReadyType, Step as ProgressStep};
pub use s3::{Service as S3Service, S3};
pub use settings::Settings;
pub use storage::Storage;
pub use warehouse::Warehouse;

pub mod cache;
pub mod chain;
pub mod db;
pub mod errors;
pub mod models;
pub mod progress;
pub mod s3;
pub mod settings;
pub mod storage;
pub mod utils;
pub mod warehouse;

mod banner;

static EMOJI_SETUP: Emoji<'_, '_> = Emoji("💾  ", "");
static EMOJI_MIGRATIONS: Emoji<'_, '_> = Emoji("🚐  ", "");
static EMOJI_NETWORKS: Emoji<'_, '_> = Emoji("📢  ", "");
static EMOJI_READY: Emoji<'_, '_> = Emoji("🟢  ", "");
static EMOJI_QUIT: Emoji<'_, '_> = Emoji("🛑  ", "");

pub const INDEXER_PROMOTION: u64 = 20;
pub const INDEXER_HEARTBEAT: u64 = 2;

pub type Warnings = Vec<String>;
pub type BlockHeight = u64;
pub type RateLimiter = GovernorRateLimiter<NotKeyed, InMemoryState, DefaultClock>;

#[derive(Clone)]
pub struct App {
	pub uuid: Uuid,
	pub networks: Arc<RwLock<HashMap<PrimaryId, Arc<BoxedChain>>>>,
	pub settings: Arc<Settings>,
	pub cache: Arc<RwLock<Cache>>,
	pub storage: Arc<Storage>,
	db: Arc<Db>,
	pub warehouse: Arc<Warehouse>,
	is_ready: Arc<AtomicBool>,
	is_primary: Arc<AtomicBool>,
	connected_at: Arc<RwLock<Option<NaiveDateTime>>>,
	pub num_cpus: usize,
}

impl App {
	pub async fn new(
		settings: Arc<Settings>,
		cache: Arc<RwLock<Cache>>,
		storage: Arc<Storage>,
		db: Arc<Db>,
		warehouse: Arc<Warehouse>,
	) -> Result<Self> {
		let mut app = App {
			uuid: utils::new_uuid(),
			networks: Arc::new(RwLock::new(HashMap::new())),
			settings,
			cache,
			storage,
			db,
			warehouse,
			is_ready: Arc::new(AtomicBool::new(false)),
			is_primary: Arc::new(AtomicBool::new(false)),
			connected_at: Arc::new(RwLock::new(None)),
			num_cpus: num_cpus::get(),
		};

		app.networks = Arc::new(RwLock::new(app.get_networks().await?));

		Ok(app)
	}

	pub fn db(&self) -> &DatabaseConnection {
		self.db.get()
	}

	pub async fn db_tx(&self) -> Result<DatabaseTransaction> {
		Ok(self.db().begin().await?)
	}

	pub async fn get_networks(&self) -> Result<HashMap<PrimaryId, Arc<BoxedChain>>> {
		let mut ret = HashMap::new();

		for n in
			Network::get_all_by_env(self.db(), self.settings.env, Some(false)).await?.into_iter()
		{
			let network_id = n.network_id;
			let c = self.cache.clone();

			let boxed_chain: BoxedChain = match n.architecture {
				Architecture::Bitcoin => Box::new(Bitcoin::new(c, n)),
				Architecture::Evm => Box::new(Evm::new(c, n)),
			};

			ret.insert(network_id, Arc::new(boxed_chain));
		}

		Ok(ret)
	}

	pub async fn should_reconnect(&self) -> Result<bool> {
		Ok(match *self.connected_at.read().await {
			Some(connected_at) => {
				let networks_updated_at =
					Config::get::<_, u8>(self.db(), ConfigKey::NetworksUpdated)
						.await?
						.map(|v| v.updated_at)
						.unwrap_or_else(utils::now);

				connected_at < networks_updated_at
			}
			None => true,
		})
	}

	pub async fn connect_networks(&self, silent: bool) -> Result<()> {
		let template = format!(
			"       {{spinner}}  {} {{prefix:.bold}}: {{wide_msg:.bold.dim}}",
			style("↳").bold().dim()
		);
		let spinner_style =
			ProgressStyle::with_template(&template).unwrap().tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

		let m = MultiProgress::new();

		let mut threads = vec![];
		for n in
			Network::get_all_by_env(self.db(), self.settings.env, Some(false)).await?.into_iter()
		{
			let pb = m.add(ProgressBar::new(1_000_000));
			pb.set_style(spinner_style.clone());
			pb.set_prefix(n.name.clone());
			pb.enable_steady_tick(Duration::from_millis(50));

			threads.push({
				let c = self.cache.clone();

				tokio::spawn({
					let mut boxed_chain: BoxedChain = match n.architecture {
						Architecture::Bitcoin => Box::new(Bitcoin::new(c, n.clone())),
						Architecture::Evm => Box::new(Evm::new(c, n.clone())),
					};

					async move {
						if !silent {
							pb.set_message("connecting…");
						}

						if boxed_chain.connect().await? {
							if !silent {
								pb.finish_with_message(format!(
									"connected to {}",
									utils::with_masked_auth(&boxed_chain.get_rpc().unwrap())
								));
							}

							Ok(Arc::new(boxed_chain))
						} else {
							if !silent {
								pb.finish_with_message("could not connect");
							}

							Err(eyre!("{}: Could not connect to an RPC endpoint.", n.name))
						}
					}
				})
			});
		}

		let (connected_networks, failures): (HashMap<_, _>, Vec<_>) =
			join_all(threads).await.into_iter().partition_map(|r| match r.unwrap() {
				Ok(chain) => {
					let network_id = chain.get_network().network_id;
					Either::Left((network_id, chain))
				}
				Err(e) => Either::Right(e),
			});

		if !failures.is_empty() {
			bail!(failures.iter().map(|e| format!("- {e}")).join("\n"));
		}

		let mut networks = self.networks.write().await;
		*networks = connected_networks;

		let mut connected_at = self.connected_at.write().await;
		*connected_at = Some(utils::now());

		Ok(())
	}

	pub async fn get_warnings(&self) -> Result<Warnings> {
		let mut warnings = Warnings::new();

		let networks = self.networks.read().await;
		if networks.is_empty() {
			warnings.push("No active networks found".to_string());
		} else {
			warnings.extend(
				networks
					.iter()
					.filter_map(|(_, chain)| {
						if self.settings.is_indexer && chain.get_network().rps == 0 {
							Some(format!(
								"{} rpc requests are not rate-limited",
								chain.get_network().name
							))
						} else {
							None
						}
					})
					.collect::<Warnings>(),
			);
		}

		Ok(warnings)
	}

	pub fn is_leading(&self) -> bool {
		self.is_ready() && self.is_primary()
	}

	pub fn is_ready(&self) -> bool {
		self.is_ready.load(Ordering::SeqCst)
	}

	pub fn set_is_ready(&self) {
		self.is_ready.store(true, Ordering::SeqCst);
	}

	pub fn is_primary(&self) -> bool {
		self.is_primary.load(Ordering::SeqCst)
	}

	pub async fn set_is_primary(&self, is_primary: bool) -> Result<()> {
		if is_primary != self.is_primary() {
			self.cache.write().await.set_read_only(!is_primary).await?;
			self.is_primary.store(is_primary, Ordering::SeqCst);
		}

		Ok(())
	}

	pub async fn format_address(&self, address: &str) -> Result<String> {
		for (_, chain) in self.networks.read().await.iter() {
			let formatted_address = chain.format_address(address);
			if formatted_address != address {
				return Ok(formatted_address);
			}
		}

		Ok(address.to_string())
	}
}

#[derive(Display, Debug, Serialize, Deserialize)]
pub enum IdPrefix {
	#[display(fmt = "net")]
	Network,
	#[display(fmt = "key")]
	ApiKey,
	#[display(fmt = "ent")]
	Entity,
	#[display(fmt = "adr")]
	Address,
	#[display(fmt = "tag")]
	Tag,
}

#[derive(
	Default, Debug, EnumIter, DeriveActiveEnum, Copy, Clone, PartialEq, Eq, Serialize, Deserialize,
)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum Env {
	#[serde(rename = "localhost")]
	Localhost = 1,
	#[serde(rename = "testnet")]
	Testnet = 2,
	#[serde(rename = "mainnet")]
	#[default]
	Mainnet = 3,
}

impl ValueEnum for Env {
	fn value_variants<'a>() -> &'a [Self] {
		&[Self::Localhost, Self::Testnet, Self::Mainnet]
	}

	fn to_possible_value<'a>(&self) -> Option<PossibleValue> {
		match self {
			Self::Localhost => Some(PossibleValue::new("localhost")),
			Self::Testnet => Some(PossibleValue::new("testnet")),
			Self::Mainnet => Some(PossibleValue::new("mainnet")),
		}
	}
}

#[derive(
	Default, Debug, EnumIter, DeriveActiveEnum, Copy, Clone, PartialEq, Eq, Serialize, Deserialize,
)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum Architecture {
	#[serde(rename = "bitcoin")]
	#[default]
	Bitcoin = 1,
	#[serde(rename = "evm")]
	Evm = 2,
}

pub fn quit(app_error: AppError) -> ! {
	println!("{} {}Shutting down…\n\n› {}", style("[err]").bold().dim(), EMOJI_QUIT, app_error);

	process::exit(match app_error {
		AppError::SignalHandler | AppError::ServerStartup { .. } => exitcode::OSERR,
		AppError::Config { .. } => exitcode::CONFIG,
		_ => exitcode::UNAVAILABLE,
	})
}
