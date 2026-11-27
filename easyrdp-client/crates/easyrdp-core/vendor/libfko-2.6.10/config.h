#ifndef CONFIG_H
#define CONFIG_H 1

/*
 * EasyRDP vendor config.h for libfko 2.6.10 (hand-written, dual-platform).
 *
 * Upstream libfko normally gets its config.h from GNU autoconf. To build the
 * library directly with the Rust `cc` crate (MSVC on Windows / gcc-clang on
 * Linux), we ship this fixed configuration instead. It mirrors what the
 * upstream `./configure` + shipped win32/config.h produce for the client-only
 * build (no GPG, no FIU, no cunit). Build scripts define HAVE_CONFIG_H so
 * fko_common.h / common.h pick this file up.
 */

/* --- universal --- */
#define STDC_HEADERS 1
#define HAVE_CTYPE_H 1
#define HAVE_ERRNO_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_MEMSET 1
#define HAVE_MEMMOVE 1
#define HAVE_STRDUP 1
#define HAVE_STRCHR 1
#define HAVE_STRRCHR 1
#define HAVE_STRSPN 1
#define HAVE_STRCSPN 1
#define HAVE_STRNLEN 1
#define HAVE_STRNCASECMP 1

/* GPG / FIU / test hooks: intentionally disabled (client-only build). */
/* HAVE_LIBGPGME, HAVE_LIBFIU, HAVE_C_UNIT_TESTS stay undefined. */

#ifdef _WIN32
/* Windows (MSVC / MinGW): mirrors fwknop-2.6.10 win32/config.h client build.
 * HAVE_STRNDUP is also satisfied by fko_util.c's own WIN32 fallback. */
#define HAVE_STRNDUP 1
#define VERSION "2.6.10"
#else
/* Linux / macOS */
#define HAVE_STDINT_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_UNISTD_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_TIME_H 1
#define TIME_WITH_SYS_TIME 1
#define HAVE_ENDIAN_H 1
#define HAVE_STRNDUP 1
#define VERSION "2.6.10"
#endif

#endif /* CONFIG_H */
