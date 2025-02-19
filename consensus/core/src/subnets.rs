use std::fmt::{Debug, Display, Formatter};
use std::str::{self, FromStr};

use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use serde::{Deserialize, Serialize};

/// The size of the array used to store subnetwork IDs.
pub const SUBNETWORK_ID_SIZE: usize = 20;

/// The domain representation of a Subnetwork ID
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, BorshSerialize, BorshDeserialize, BorshSchema)]
pub struct SubnetworkId([u8; SUBNETWORK_ID_SIZE]);

impl AsRef<[u8]> for SubnetworkId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl SubnetworkId {
    pub const fn from_byte(b: u8) -> SubnetworkId {
        let mut bytes = [0u8; SUBNETWORK_ID_SIZE];
        bytes[0] = b;
        SubnetworkId(bytes)
    }

    pub const fn from_bytes(bytes: [u8; SUBNETWORK_ID_SIZE]) -> SubnetworkId {
        SubnetworkId(bytes)
    }

    /// Returns true if the subnetwork is a built-in subnetwork, which
    /// means all nodes, including partial nodes, must validate it, and its transactions
    /// always use 0 gas.
    #[inline]
    pub fn is_builtin(&self) -> bool {
        *self == SUBNETWORK_ID_COINBASE || *self == SUBNETWORK_ID_REGISTRY
    }

    /// Returns true if the subnetwork is the native or a built-in subnetwork
    #[inline]
    pub fn is_builtin_or_native(&self) -> bool {
        *self == SUBNETWORK_ID_NATIVE || self.is_builtin()
    }
}

impl Display for SubnetworkId {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut hex = [0u8; SUBNETWORK_ID_SIZE * 2];
        faster_hex::hex_encode(&self.0, &mut hex).expect("The output is exactly twice the size of the input");
        f.write_str(str::from_utf8(&hex).expect("hex is always valid UTF-8"))
    }
}

impl FromStr for SubnetworkId {
    type Err = faster_hex::Error;

    #[inline]
    fn from_str(str: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; SUBNETWORK_ID_SIZE];
        faster_hex::hex_decode(str.as_bytes(), &mut bytes)?;
        Ok(SubnetworkId(bytes))
    }
}

/// The default subnetwork ID which is used for transactions without related payload data
pub const SUBNETWORK_ID_NATIVE: SubnetworkId = SubnetworkId::from_byte(0);

/// The subnetwork ID which is used for the coinbase transaction
pub const SUBNETWORK_ID_COINBASE: SubnetworkId = SubnetworkId::from_byte(1);

/// The subnetwork ID which is used for adding new sub networks to the registry
pub const SUBNETWORK_ID_REGISTRY: SubnetworkId = SubnetworkId::from_byte(2);
