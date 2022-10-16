use chrono::{offset::Utc, NaiveDateTime};
use nanoid::nanoid;

use crate::IdPrefix;

pub fn new_unique_id(prefix: IdPrefix) -> String {
	unique_id(
		prefix,
		&nanoid!(
			8,
			&[
				'2', '3', '4', '5', '6', '7', '8', '9', 'a', 'c', 'd', 'e',
				'g', 'h', 'j', 'k', 'm', 'n', 'q', 'r', 's', 't', 'v', 'w',
				'x', 'z',
			]
		),
	)
}

pub fn unique_id(prefix: IdPrefix, id: &str) -> String {
	format!("{prefix}_{id}")
}

pub fn now() -> NaiveDateTime {
	Utc::now().naive_utc()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_unique_id() {
		assert_eq!(
			unique_id(IdPrefix::SanctionedAddress, "id"),
			format!("{}_id", IdPrefix::SanctionedAddress)
		);
	}
}
