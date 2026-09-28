//! CCSDS and ECSS PUS codecs, host-agnostic: the space packet, the PUS
//! secondary headers, TM/TC/AOS/USLP transfer frames and the CLCW, the
//! sync marker, randomizer, CLTUs and BCH code blocks, Reed-Solomon
//! (255,223) decoding, time codes, CFDP PDU headers, and splitting a
//! recording into packets. Every decoder builds a tree of `Field`s -- a
//! name, the bits it covers, its raw value, what it means, and whether it
//! checks out -- so the GUI draws every layer the same way; every layer
//! also encodes, so a test vector round-trips.
//!
//! Nothing here knows the MIB: identifying a packet and decoding its
//! parameters is `fenix-mib`'s, on top of this.

pub mod bits;
pub mod cfdp;
pub mod coding;
pub mod crc;
pub mod field;
pub mod frames;
pub mod hex;
pub mod packet;
pub mod pus;
pub mod rs;
pub mod stream;
pub mod time;

pub use field::{Check, Field, Link};
pub use packet::{Packet, PrimaryHeader};
pub use pus::{Profile, PusEdition};
