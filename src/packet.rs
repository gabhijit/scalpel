//! Packet Structure

use core::cell::RefCell;
use core::fmt::Debug;

// FIXME: Should work with `no_std`
use std::collections::HashMap;
use std::sync::RwLock;

use lazy_static::lazy_static;

use crate::types::{EncapType, LayerCreatorFn, ENCAP_TYPE_ETH};
use crate::Error;
use crate::{FakeLayer, Layer};

lazy_static! {
    static ref ENCAP_TYPES_MAP: RwLock<HashMap<EncapType, LayerCreatorFn>> =
        RwLock::new(HashMap::new());
}

#[derive(Debug, Default)]
struct Timestamp {
    secs: i64,
    nsecs: i64,
}

/// [`Packet`] is a central structure in `scalpel` containing the decoded data and some metadata.
///
/// When a byte-stream is 'dissected' by scalpel, it creates a `Packet` structure that contains the
/// following information.
///  * `data` : Optional 'data' from which this packet is constructed.
///  * `meta` : Metadata associated with the packet. This contains information like timestamp,
///             interface identifier where the data was captured etc. see `PacketMetadata` for
///             details.
///  * `layers`: A Vector of Opaque structures, each implementing the `Layer` trait. For example
///              Each of the following is a Layer - `Ethernet`, `IPv4`, `TCP` etc.
///  * `unprocessed`: The partof the original byte-stream that is not processed and captured into
///                   `layers` above.
#[derive(Debug, Default)]
pub struct Packet<'a> {
    data: Option<&'a [u8]>,
    meta: PacketMetadata,
    layers: Vec<Box<dyn Layer>>,
    unprocessed: Vec<u8>,
}

/// Metadata associated with the Packet.
#[derive(Debug, Default)]
pub struct PacketMetadata {
    /// Capture timestamp
    timestamp: Timestamp,
    /// Interface ID
    iface: i8,
    /// Actual length on the wire
    len: u16,
    /// Capture length
    caplen: u16,
}

impl<'a> Packet<'a> {
    /// Register a new Layer 2 encoding
    ///
    /// In order to dissect bytes on wire into a [`Packet`] structure, the right encoding needs to
    /// be registered. An internal Map of [`EncapType`] -> [`LayerCreatorFn`] is updated when a new
    /// Layer 2 registers itself. This will cause the 'decoder' function for that layer.
    pub fn register_encap_type(encap: EncapType, creator: LayerCreatorFn) -> Result<(), Error> {
        let mut map = ENCAP_TYPES_MAP.write().unwrap();
        if map.contains_key(&ENCAP_TYPE_ETH) {
            return Err(Error::RegisterError);
        }
        map.insert(encap, creator);

        Ok(())
    }

    /// Create a [`Packet`] from a u8 slice.
    ///
    /// This is the main 'decoder' function. An application would typically call `Packet::from_u8`.
    /// This would then return a [`Packet`] structure. The application can then perform actions if
    /// any on the returned structure.
    pub fn from_u8(bytes: &'a [u8], encap: EncapType) -> Result<Self, Error> {
        let mut p = Packet::default();

        let l2: Box<dyn Layer>;
        {
            let map = ENCAP_TYPES_MAP.read().unwrap();
            let creator_fn = map.get(&encap);
            if creator_fn.is_none() {
                let new: Vec<_> = bytes.into();
                let _ = core::mem::replace(&mut p.unprocessed, new);

                return Ok(p);
            }

            l2 = creator_fn.unwrap()();
        }

        let layer: RefCell<Box<dyn Layer>> = RefCell::new(l2);
        let mut res: (Option<Box<dyn Layer>>, usize);
        let mut start = 0;
        loop {
            {
                let mut decode_layer = layer.borrow_mut();
                res = decode_layer.from_u8(&bytes[start..])?;
            }

            if res.0.is_none() {
                let fake_boxed = Box::new(FakeLayer {});
                let boxed = layer.replace(fake_boxed);

                p.layers.push(boxed);
                start += res.1;
                break;
            }

            // if the layer exists, get it in a layer.
            let boxed = layer.replace(res.0.unwrap());
            start += res.1;

            // append the layer to layers.
            p.layers.push(boxed);
        }
        if start != bytes.len() {
            let new: Vec<_> = bytes[start..].into();
            let _ = core::mem::replace(&mut p.unprocessed, new);
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use hex;

    #[test]
    fn from_u8_fail_too_short() {
        let _ = crate::layers::register_defaults();

        let p = Packet::from_u8("".as_bytes(), ENCAP_TYPE_ETH);

        assert!(p.is_err(), "{:?}", p.ok());
    }

    #[test]
    fn from_u8_success_eth_hdr_size() {
        let _ = crate::layers::register_defaults();

        let p = Packet::from_u8(&[0; 14], ENCAP_TYPE_ETH);

        assert!(p.is_ok(), "{:?}", p.err());
    }

    #[test]
    fn parse_valid_ipv4_packet() {
        use crate::layers;
        use crate::layers::ethernet::ETH_HEADER_LEN;
        use crate::layers::ipv4::IPV4_BASE_HDR_LEN;

        let _ = layers::register_defaults();

        let array = hex::decode("00e08100b02800096b88f5c90800450000c1d24940008006c85b0a000005cf2e865e0cc30050a80076877de014025018faf0ad62000048454144202f76342f69756964656e742e6361623f3033303730313132303820485454502f312e310d0a4163636570743a202a2f2a0d0a557365722d4167656e743a20496e6475737472792055706461746520436f6e74726f6c0d0a486f73743a2077696e646f77737570646174652e6d6963726f736f66742e636f6d0d0a436f6e6e656374696f6e3a204b6565702d416c6976650d0a0d0a");
        assert!(array.is_ok());

        let array = array.unwrap();
        let len = array.len();
        let p = Packet::from_u8(&array, ENCAP_TYPE_ETH);
        assert!(p.is_ok(), "{:?}", p.err());

        let p = p.unwrap();
        assert!(p.layers.len() == 2, "{:?}", p);
        assert!(
            p.unprocessed.len() == (len - (ETH_HEADER_LEN + IPV4_BASE_HDR_LEN)),
            "{}:{}:{:?}",
            len,
            p.unprocessed.len(),
            p
        );
    }

    #[test]
    fn parse_valid_ipv6_packet() {
        use crate::layers;
        use crate::layers::ethernet::ETH_HEADER_LEN;
        use crate::layers::ipv6::IPV6_BASE_HDR_LEN;

        let _ = layers::register_defaults();

        let array = hex::decode("000573a007d168a3c4f949f686dd600000000020064020010470e5bfdead49572174e82c48872607f8b0400c0c03000000000000001af9c7001903a088300000000080022000da4700000204058c0103030801010402");
        assert!(array.is_ok());
        assert!(true);

        let array = array.unwrap();
        let len = array.len();
        let p = Packet::from_u8(&array, ENCAP_TYPE_ETH);
        assert!(p.is_ok(), "{:?}", p.err());

        let p = p.unwrap();
        assert!(p.layers.len() == 2, "{:?}", p);
        assert!(
            p.unprocessed.len() == (len - (ETH_HEADER_LEN + IPV6_BASE_HDR_LEN)),
            "{}:{}:{:?}",
            len,
            p.unprocessed.len(),
            p
        );
    }
}
