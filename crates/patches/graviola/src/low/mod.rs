// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[macro_use]
mod macros;

pub(crate) mod ct;

#[cfg(doc)]
pub mod inline_assembly_safety;

mod generic {
    pub(super) mod blockwise;
    #[cfg(target_arch = "aarch64")]
    pub(crate) mod chacha20;
    pub(super) mod ct_copy;
    pub(super) mod ct_equal;
    #[cfg(test)]
    pub(crate) mod ghash;
    #[cfg(target_arch = "aarch64")]
    pub(crate) mod mlkem;
    pub(crate) mod poly1305;
    #[cfg(target_arch = "x86_64")]
    pub(super) mod sha256;
    pub(super) mod sha512;
    pub(super) mod zeroise;
}

mod entry;
mod posint;

pub(crate) use entry::Entry;
pub(crate) use generic::blockwise::Blockwise;
pub(crate) use generic::ct_copy::{ct_copy, ct_select_i16};
pub(crate) use generic::ct_equal::ct_equal;
pub(crate) use generic::poly1305;
pub(crate) use generic::zeroise::{zeroise, zeroise_value};
pub(crate) use posint::{PosInt, SecretPosInt};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub(in crate::low) use x86_64::cpu::{enter_cpu_state, zero_bytes, ct_compare_bytes, leave_cpu_state, verify_cpu_features};
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::chacha20;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::aes::AesKey;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::aes_gcm;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_add::bignum_add;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_add_p256::bignum_add_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_add_p384::bignum_add_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_bitsize::bignum_bitsize;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_cmp_lt::bignum_cmp_lt;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_coprime::bignum_coprime;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_copy_row_from_table_mux::bignum_copy_row_from_table;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_ctz::bignum_ctz;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_demont::bignum_demont;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_point_select_p256::{bignum_aff_point_select_p256, bignum_jac_point_select_p256};
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_point_select_p384::bignum_jac_point_select_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_demont_p256::bignum_demont_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_demont_p384::bignum_demont_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_digitsize::bignum_digitsize;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_emontredc_8n::bignum_emontredc_8n;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_eq::bignum_eq;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_inv_p256::bignum_inv_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_inv_p384::bignum_inv_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_kmul_16_32::bignum_kmul_16_32;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_kmul_32_64::bignum_kmul_32_64;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_ksqr_16_32::bignum_ksqr_16_32;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_ksqr_32_64::bignum_ksqr_32_64;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_madd_n25519::bignum_madd_n25519;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_mod_n25519::bignum_mod_n25519;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_mod_n256::bignum_mod_n256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_mod_n384::bignum_mod_n384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_modadd::bignum_modadd;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_modinv::bignum_modinv;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_modsub::bignum_modsub;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montifier::bignum_montifier;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montmul::bignum_montmul;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montmul_p256::bignum_montmul_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montmul_p384::bignum_montmul_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montredc::bignum_montredc;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montsqr::bignum_montsqr;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montsqr_p256::bignum_montsqr_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_montsqr_p384::bignum_montsqr_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_mul::bignum_mul;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_mux::bignum_mux;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_neg_p25519::bignum_neg_p25519;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_neg_p256::bignum_neg_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_neg_p384::bignum_neg_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_negmodinv::bignum_negmodinv;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_optsub::bignum_optsub;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_shr_small::bignum_shr_small;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_tomont_p256::bignum_tomont_p256;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::bignum_tomont_p384::bignum_tomont_p384;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::curve25519_x25519::curve25519_x25519;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::curve25519_x25519base::curve25519_x25519base;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::edwards25519_decode::edwards25519_decode;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::edwards25519_scalarmulbase::edwards25519_scalarmulbase;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::edwards25519_scalarmuldouble::edwards25519_scalarmuldouble;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::ghash;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem::{
    mlkem_ntt,
    mlkem_intt,
    mlkem_mulcache_compute,
    mlkem_rej_uniform_vartime
};
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_basemul_k3::mlkem_basemul_k3;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_reduce::mlkem_reduce;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_frombytes::mlkem_frombytes;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_tobytes::mlkem_tobytes;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_tomont::mlkem_tomont;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::mlkem_unpack::mlkem_unpack;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::p256_montjadd::p256_montjadd;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::p256_montjdouble::p256_montjdouble;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::p256_montjmixadd::p256_montjmixadd;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::p384_montjadd::p384_montjadd;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::p384_montjdouble::p384_montjdouble;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::sha256_mux::sha256_compress_blocks;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::sha512_mux::sha512_compress_blocks;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::sha3_keccak_f1600::sha3_keccak_f1600;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::sha3_keccak4_f1600_shim::sha3_keccak4_f1600;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::sha3_keccak2of4_f1600::sha3_keccak2of4_f1600;
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub(in crate::low) use aarch64::cpu::{enter_cpu_state, zero_bytes, ct_compare_bytes, leave_cpu_state, verify_cpu_features};
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::aes::AesKey;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::aes_gcm;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_add::bignum_add;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_add_p256::bignum_add_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_add_p384::bignum_add_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_bitsize::bignum_bitsize;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_cmp_lt::bignum_cmp_lt;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_coprime::bignum_coprime;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_copy_row_from_table_mux::bignum_copy_row_from_table;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_ctz::bignum_ctz;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_point_select_p256::{bignum_aff_point_select_p256, bignum_jac_point_select_p256};
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_point_select_p384::bignum_jac_point_select_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_demont::bignum_demont;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_demont_p256::bignum_demont_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_demont_p384::bignum_demont_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_digitsize::bignum_digitsize;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_emontredc_8n::bignum_emontredc_8n;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_eq::bignum_eq;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_inv_p256::bignum_inv_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_inv_p384::bignum_inv_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_kmul_16_32::bignum_kmul_16_32;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_kmul_32_64::bignum_kmul_32_64;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_ksqr_16_32::bignum_ksqr_16_32;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_ksqr_32_64::bignum_ksqr_32_64;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_madd_n25519::bignum_madd_n25519;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_mod_n25519::bignum_mod_n25519;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_mod_n256::bignum_mod_n256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_mod_n384::bignum_mod_n384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_modadd::bignum_modadd;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_modinv::bignum_modinv;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_modsub::bignum_modsub;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montifier::bignum_montifier;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montmul::bignum_montmul;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montmul_p256::bignum_montmul_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montmul_p384::bignum_montmul_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montredc::bignum_montredc;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montsqr::bignum_montsqr;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montsqr_p256::bignum_montsqr_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_montsqr_p384::bignum_montsqr_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_mul::bignum_mul;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_mux::bignum_mux;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_neg_p25519::bignum_neg_p25519;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_neg_p256::bignum_neg_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_neg_p384::bignum_neg_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_negmodinv::bignum_negmodinv;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_optsub::bignum_optsub;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_shr_small::bignum_shr_small;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_tomont_p256::bignum_tomont_p256;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::bignum_tomont_p384::bignum_tomont_p384;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::curve25519_x25519::curve25519_x25519;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::curve25519_x25519base::curve25519_x25519base;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::edwards25519_decode::edwards25519_decode;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::edwards25519_scalarmulbase::edwards25519_scalarmulbase;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::edwards25519_scalarmuldouble::edwards25519_scalarmuldouble;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::mlkem::{
    mlkem_ntt,
    mlkem_intt,
    mlkem_mulcache_compute,
    mlkem_rej_uniform_vartime,
};
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::mlkem_basemul_k3::mlkem_basemul_k3;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::mlkem_reduce::mlkem_reduce;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::mlkem_tobytes::mlkem_tobytes;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::mlkem_tomont::mlkem_tomont;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::ghash;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::p256_montjadd::p256_montjadd;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::p256_montjdouble::p256_montjdouble;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::p256_montjmixadd::p256_montjmixadd;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::p384_montjadd::p384_montjadd;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::p384_montjdouble::p384_montjdouble;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::sha256::sha256_compress_blocks;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::sha3_keccak_f1600_mux::sha3_keccak_f1600;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::sha3_keccak4_f1600_mux::sha3_keccak4_f1600;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::sha3_keccak2of4_f1600::sha3_keccak2of4_f1600;
#[cfg(target_arch = "aarch64")]
pub(crate) use generic::chacha20;
#[cfg(target_arch = "aarch64")]
pub(crate) use generic::sha512::sha512_compress_blocks;
#[cfg(target_arch = "aarch64")]
pub(crate) use generic::mlkem::{mlkem_frombytes, mlkem_unpack};
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("This crate only supports x86_64 or aarch64");
