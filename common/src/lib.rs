use clap::{builder, ValueEnum};
use derive_more::Display;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};

pub mod models;
pub mod progress;
pub mod utils;

pub mod db;
pub use db::Db;

pub mod errors;
pub use errors::AppError;

pub mod address;
pub use address::Address;

pub mod warehouse;
pub use warehouse::Clickhouse;

pub mod settings;
pub use settings::Settings;

#[derive(Clone)]
pub struct AppState {
	pub uuid: Uuid,
	pub settings: Arc<Settings>,
	pub warehouse: Arc<Clickhouse>,
	pub db: Arc<Db>,
	pub env: Env,
}

impl AppState {
	pub fn new(
		settings: Arc<Settings>,
		warehouse: Arc<Clickhouse>,
		db: Arc<Db>,
		env: Env,
	) -> Self {
		AppState { uuid: utils::new_uuid(), settings, warehouse, db, env }
	}
}

#[derive(Display, Debug, Serialize, Deserialize)]
pub enum IdPrefix {
	#[display(fmt = "net")]
	Network,
	#[display(fmt = "key")]
	ApiKey,
	#[display(fmt = "acc")]
	Account,
	#[display(fmt = "lab")]
	Label,
	#[display(fmt = "lab_adr")]
	LabeledAddress,
	#[display(fmt = "txn")]
	Transaction,
}

#[derive(Display, Debug, PartialEq, Eq)]
pub enum LabelId {
	#[display(fmt = "lab_ofac")]
	Ofac,
	#[display(fmt = "lab_ofsi")]
	Ofsi,
}

impl FromStr for LabelId {
	type Err = ();
	fn from_str(id: &str) -> Result<LabelId, Self::Err> {
		match id {
			"lab_ofac" => Ok(LabelId::Ofac),
			"lab_ofsi" => Ok(LabelId::Ofsi),
			_ => Err(()),
		}
	}
}

#[derive(
	Debug,
	EnumIter,
	DeriveActiveEnum,
	Copy,
	Clone,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum Env {
	Localhost = 1,
	Testnet = 2,
	Mainnet = 3,
}

impl ValueEnum for Env {
	fn value_variants<'a>() -> &'a [Self] {
		&[Self::Localhost, Self::Testnet, Self::Mainnet]
	}

	fn to_possible_value<'a>(&self) -> Option<builder::PossibleValue> {
		match self {
			Self::Localhost => Some(builder::PossibleValue::new("localhost")),
			Self::Testnet => Some(builder::PossibleValue::new("testnet")),
			Self::Mainnet => Some(builder::PossibleValue::new("mainnet")),
		}
	}
}

#[derive(
	Debug,
	EnumIter,
	DeriveActiveEnum,
	Copy,
	Clone,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum Blockchain {
	#[serde(rename = "bitcoin")]
	Bitcoin = 1,
	#[serde(rename = "evm")]
	Evm = 2,
}

#[derive(Serialize, Deserialize)]
pub enum Risk {
	#[serde(rename = "LOW")]
	Low = 10,
	#[serde(rename = "MEDIUM")]
	Medium = 20,
	#[serde(rename = "HIGH")]
	High = 30,
	#[serde(rename = "SEVERE")]
	Severe = 40,
}
