//! Build script: vendor libfko 2.6.10 (fwknop official C library) directly
//! into the easyrdp-core static library via the `cc` crate.
//!
//! Source layout (vendor/libfko-2.6.10/):
//! - lib/  : libfko core (SPA encode/decode, Rijndael-256, HMAC, digests)
//! - common/: fko_util + strlcpy/strlcat (shared upstream helpers)
//! - config.h: hand-written dual-platform config (see file header)
//! Excluded upstream: server/, client/, gpgme (HAVE_LIBGPGME off), cunit tests.
//!
//! Windows MSVC specifics:
//! - define WIN32 so upstream's win32 branches (getlogin.h, _ftime_s, ...) activate
//! - vendor fko_common.h was minimally patched (dropped _snprintf alias and
//!   hand-rolled uint typedefs; see comment in the file)
//! - link ws2_32 for fko_util.c ipv4_resolve (WSAStartup/getaddrinfo)

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let root = "vendor/libfko-2.6.10";
    println!("cargo:rerun-if-changed={root}");

    let mut files = [
        "base64.c",
        "cipher_funcs.c",
        "digest.c",
        "fko_client_timeout.c",
        "fko_decode.c",
        "fko_digest.c",
        "fko_encode.c",
        "fko_encryption.c",
        "fko_error.c",
        "fko_funcs.c",
        "fko_hmac.c",
        "fko_message.c",
        "fko_nat_access.c",
        "fko_rand_value.c",
        "fko_server_auth.c",
        "fko_timestamp.c",
        "fko_user.c",
        "hmac.c",
        "md5.c",
        "rijndael.c",
        "sha1.c",
        "sha2.c",
        "sha3.c",
    ]
    .iter()
    .map(|f| format!("{root}/{f}"))
    .collect::<Vec<_>>();
    files.push(format!("{root}/common/fko_util.c"));
    // strlcpy/strlcat: MSVC has neither; glibc < 2.38 has neither. Compiling
    // the upstream fallbacks unconditionally is safe on every target.
    files.push(format!("{root}/common/strlcpy.c"));
    files.push(format!("{root}/common/strlcat.c"));

    let mut build = cc::Build::new();
    build
        .include(root)
        .include(format!("{root}/common"))
        .define("HAVE_CONFIG_H", None)
        .files(files.iter().map(|s| s.as_str()));

    if target.contains("windows") {
        build
            .define("WIN32", None)
            .file(format!("{root}/getlogin.c"));
        println!("cargo:rustc-link-lib=dylib=ws2_32");
    }

    build.compile("fko");
}
