//! Certificate compression and decompression support
//!
//! This crate supports compression and decompression everywhere
//! certificates are used, in accordance with [RFC8879][rfc8879].
//!
//! Note that this is only supported for TLS1.3 connections.
//!
//! # Getting started
//!
//! Build this crate with the `brotli` and/or `zlib` crate features.  This
//! adds dependencies on these crates.  They are used by default if enabled.
//!
//! We especially recommend `brotli` as it has the widest deployment so far.
//!
//! # Custom compression/decompression implementations
//!
//! 1. Implement the [`CertCompressor`] and/or [`CertDecompressor`] traits
//! 2. Provide those to:
//!   - [`ClientConfig::cert_compressors`][cc_cc] or [`ServerConfig::cert_compressors`][sc_cc].
//!   - [`ClientConfig::cert_decompressors`][cc_cd] or [`ServerConfig::cert_decompressors`][sc_cd].
//!
//! These are used in these circumstances:
//!
//! | Peer | Client authentication | Server authentication |
//! | ---- | --------------------- | --------------------- |
//! | *Client* | [`ClientConfig::cert_compressors`][cc_cc] | [`ClientConfig::cert_decompressors`][cc_cd] |
//! | *Server* | [`ServerConfig::cert_decompressors`][sc_cd] | [`ServerConfig::cert_compressors`][sc_cc] |
//!
//! [rfc8879]: https://datatracker.ietf.org/doc/html/rfc8879
//! [cc_cc]: crate::ClientConfig::cert_compressors
//! [sc_cc]: crate::ServerConfig::cert_compressors
//! [cc_cd]: crate::ClientConfig::cert_decompressors
//! [sc_cd]: crate::ServerConfig::cert_decompressors

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt::Debug;
use std::sync::Mutex;

use crate::enums::CertificateCompressionAlgorithm;
use crate::msgs::base::{Payload, PayloadU24};
use crate::msgs::codec::Codec;
use crate::msgs::handshake::{CertificatePayloadTls13, CompressedCertificatePayload};
use crate::sync::Arc;

/// Returns the supported `CertDecompressor` implementations enabled
/// by crate features.
pub fn default_cert_decompressors() -> &'static [&'static dyn CertDecompressor] {
    &[
    ]
}

/// An available certificate decompression algorithm.
pub trait CertDecompressor: Debug + Send + Sync {
    /// Decompress `input`, writing the result to `output`.
    ///
    /// `output` is sized to match the declared length of the decompressed data.
    ///
    /// `Err(DecompressionFailed)` should be returned if decompression produces more, or fewer
    /// bytes than fit in `output`, or if the `input` is in any way malformed.
    fn decompress(&self, input: &[u8], output: &mut [u8]) -> Result<(), DecompressionFailed>;

    /// Which algorithm this decompressor handles.
    fn algorithm(&self) -> CertificateCompressionAlgorithm;
}

/// Returns the supported `CertCompressor` implementations enabled
/// by crate features.
pub fn default_cert_compressors() -> &'static [&'static dyn CertCompressor] {
    &[
    ]
}

/// An available certificate compression algorithm.
pub trait CertCompressor: Debug + Send + Sync {
    /// Compress `input`, returning the result.
    ///
    /// `input` is consumed by this function so (if the underlying implementation
    /// supports it) the compression can be performed in-place.
    ///
    /// `level` is a hint as to how much effort to expend on the compression.
    ///
    /// `Err(CompressionFailed)` may be returned for any reason.
    fn compress(
        &self,
        input: Vec<u8>,
        level: CompressionLevel,
    ) -> Result<Vec<u8>, CompressionFailed>;

    /// Which algorithm this compressor handles.
    fn algorithm(&self) -> CertificateCompressionAlgorithm;
}

/// A hint for how many resources to dedicate to a compression.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum CompressionLevel {
    /// This compression is happening interactively during a handshake.
    ///
    /// Implementations may wish to choose a conservative compression level.
    Interactive,

    /// The compression may be amortized over many connections.
    ///
    /// Implementations may wish to choose an aggressive compression level.
    Amortized,
}

/// A content-less error for when `CertDecompressor::decompress` fails.
#[derive(Debug)]
pub struct DecompressionFailed;

/// A content-less error for when `CertCompressor::compress` fails.
#[derive(Debug)]
pub struct CompressionFailed;



#[allow(clippy::std_instead_of_core)] // awaits core::io::Cursor (1.97) in crate MSRV


/// An LRU cache for compressions.
///
/// The prospect of being able to reuse a given compression for many connections
/// means we can afford to spend more time on that compression (by passing
/// `CompressionLevel::Amortized` to the compressor).
#[derive(Debug)]
pub enum CompressionCache {
    /// No caching happens, and compression happens each time using
    /// `CompressionLevel::Interactive`.
    Disabled,

    /// Compressions are stored in an LRU cache.
    Enabled(CompressionCacheInner),
}

/// Innards of an enabled CompressionCache.
///
/// You cannot make one of these directly. Use [`CompressionCache::new`].
#[derive(Debug)]
pub struct CompressionCacheInner {
    /// Maximum size of underlying storage.
    size: usize,

    /// LRU-order entries.
    ///
    /// First is least-used, last is most-used.
    entries: Mutex<VecDeque<Arc<CompressionCacheEntry>>>,
}

impl CompressionCache {
    /// Make a `CompressionCache` that stores up to `size` compressed
    /// certificate messages.
    pub fn new(size: usize) -> Self {
        if size == 0 {
            return Self::Disabled;
        }

        Self::Enabled(CompressionCacheInner {
            size,
            entries: Mutex::new(VecDeque::with_capacity(size)),
        })
    }

    /// Return a `CompressionCacheEntry`, which is an owning
    /// wrapper for a `CompressedCertificatePayload`.
    ///
    /// `compressor` is the compression function we have negotiated.
    /// `original` is the uncompressed certificate message.
    pub(crate) fn compression_for(
        &self,
        compressor: &dyn CertCompressor,
        original: &CertificatePayloadTls13<'_>,
    ) -> Result<Arc<CompressionCacheEntry>, CompressionFailed> {
        match self {
            Self::Disabled => Self::uncached_compression(compressor, original),

            Self::Enabled(_) => self.compression_for_impl(compressor, original),
        }
    }

    fn compression_for_impl(
        &self,
        compressor: &dyn CertCompressor,
        original: &CertificatePayloadTls13<'_>,
    ) -> Result<Arc<CompressionCacheEntry>, CompressionFailed> {
        let (max_size, entries) = match self {
            Self::Enabled(CompressionCacheInner { size, entries }) => (*size, entries),
            _ => unreachable!(),
        };

        // context is a per-connection quantity, and included in the compressed data.
        // it is not suitable for inclusion in the cache.
        if !original.context.0.is_empty() {
            return Self::uncached_compression(compressor, original);
        }

        // cache probe:
        let encoding = original.get_encoding();
        let algorithm = compressor.algorithm();

        let mut cache = entries
            .lock()
            .map_err(|_| CompressionFailed)?;
        for (i, item) in cache.iter().enumerate() {
            if item.algorithm == algorithm && item.original == encoding {
                // this item is now MRU
                let item = cache.remove(i).unwrap();
                cache.push_back(item.clone());
                return Ok(item);
            }
        }
        drop(cache);

        // do compression:
        let uncompressed_len = encoding.len() as u32;
        let compressed = compressor.compress(encoding.clone(), CompressionLevel::Amortized)?;
        let new_entry = Arc::new(CompressionCacheEntry {
            algorithm,
            original: encoding,
            compressed: CompressedCertificatePayload {
                alg: algorithm,
                uncompressed_len,
                compressed: PayloadU24::from(Payload::new(compressed)),
            },
        });

        // insert into cache
        let mut cache = entries
            .lock()
            .map_err(|_| CompressionFailed)?;
        if cache.len() == max_size {
            cache.pop_front();
        }
        cache.push_back(new_entry.clone());
        Ok(new_entry)
    }

    /// Compress `original` using `compressor` at `Interactive` level.
    fn uncached_compression(
        compressor: &dyn CertCompressor,
        original: &CertificatePayloadTls13<'_>,
    ) -> Result<Arc<CompressionCacheEntry>, CompressionFailed> {
        let algorithm = compressor.algorithm();
        let encoding = original.get_encoding();
        let uncompressed_len = encoding.len() as u32;
        let compressed = compressor.compress(encoding, CompressionLevel::Interactive)?;

        // this `CompressionCacheEntry` in fact never makes it into the cache, so
        // `original` is left empty
        Ok(Arc::new(CompressionCacheEntry {
            algorithm,
            original: Vec::new(),
            compressed: CompressedCertificatePayload {
                alg: algorithm,
                uncompressed_len,
                compressed: PayloadU24::from(Payload::new(compressed)),
            },
        }))
    }
}

impl Default for CompressionCache {
    fn default() -> Self {
        {
            // 4 entries allows 2 certificate chains times 2 compression algorithms
            Self::new(4)
        }

    }
}

#[derive(Debug)]
pub(crate) struct CompressionCacheEntry {
    // cache key is algorithm + original:
    algorithm: CertificateCompressionAlgorithm,
    original: Vec<u8>,

    // cache value is compression result:
    compressed: CompressedCertificatePayload<'static>,
}

impl CompressionCacheEntry {
    pub(crate) fn compressed_cert_payload(&self) -> CompressedCertificatePayload<'_> {
        self.compressed.as_borrowed()
    }
}

