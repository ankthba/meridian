#!/usr/bin/env python3
"""Writes the third-party notices that ship inside Meridian.app.

The app links libmeridian_ffi.a, a static library built from the Rust crate
meridian-ffi and everything it depends on, including C and C++ code that
-sys crates compile from bundled sources. Their licenses (MIT, Apache-2.0,
BSD, ...) require their copyright and license notices to go with binary
copies. This script finds exactly what is compiled into that library and
collects the notices:

  * Rust crates: `cargo tree -p meridian-ffi --target aarch64-apple-darwin
    -e normal,no-proc-macro` gives the packages built into the library (with
    the same feature resolution as scripts/build-core.sh; build scripts,
    proc-macros and dev-dependencies run on the build machine and are not in
    the binary). `cargo metadata` gives each package's license, repository
    and source directory, whose LICENSE / COPYING / NOTICE / COPYRIGHT files
    are read.
  * Bundled native code: DuckDB's sources and third_party libraries inside
    libduckdb-sys, the SQLite amalgamation inside libsqlite3-sys, and AWS-LC
    inside aws-lc-sys (whose LICENSE file covers its bundled code).
  * The Rust standard library, which every Rust binary links.

Fails (exit 1) if any package's license is not permissive, can't be parsed,
or has no license text, and if a new native (-sys / links) crate or an
unlisted DuckDB component appears, so that it gets reviewed.

Run through scripts/third-party-notices.sh. Python standard library only.
"""

import argparse
import collections
import json
import os
import re
import subprocess
import sys
import tarfile

TARGET = "aarch64-apple-darwin"
ROOT_CRATE = "meridian-ffi"
OUTPUT = "app/Meridian/Resources/ThirdPartyNotices.txt"
CANONICAL_DIR = "scripts/licenses"

# Licenses accepted without conditions (CLAUDE.md: permissive licenses only).
PERMISSIVE = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
    "0BSD", "Unlicense", "BSL-1.0", "Unicode-3.0", "PostgreSQL", "CC0-1.0",
    "blessing", "LicenseRef-Public-Domain",
}
# File-level copyleft, accepted because the code is used unmodified and its
# source is public; the generated file says where (MPL-2.0 section 3.2(a)).
# UniFFI is the only user; it is the FFI layer the architecture is built on.
ACCEPTED_WEAK_COPYLEFT = {"MPL-2.0"}
ALLOWED_EXCEPTIONS = {"LLVM-exception"}
# Licenses whose terms need no text beyond the statement reproduced in Part 2.
NO_TEXT_NEEDED = {"blessing", "LicenseRef-Public-Domain"}
# When a package offers a choice, the first of these that it offers is used.
PREFERENCE = [
    "MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "BSL-1.0",
    "Unicode-3.0", "PostgreSQL", "MIT-0", "0BSD", "Unlicense", "CC0-1.0", "blessing",
    "LicenseRef-Public-Domain", "MPL-2.0",
]
LICENSE_NAMES = {
    "MIT": "MIT License",
    "MIT-0": "MIT No Attribution",
    "Apache-2.0": "Apache License 2.0",
    "Apache-2.0 WITH LLVM-exception": "Apache License 2.0 with LLVM Exceptions",
    "BSD-2-Clause": "BSD 2-Clause License",
    "BSD-3-Clause": "BSD 3-Clause License",
    "ISC": "ISC License",
    "Zlib": "zlib License",
    "0BSD": "Zero-Clause BSD",
    "Unlicense": "The Unlicense",
    "BSL-1.0": "Boost Software License 1.0",
    "Unicode-3.0": "Unicode License v3",
    "PostgreSQL": "PostgreSQL License",
    "MPL-2.0": "Mozilla Public License 2.0",
}

# Crates with native code or a `links` key, and how their native side is
# covered. A new one fails the run until it is reviewed and added here.
NATIVE_REVIEWED = {
    "libduckdb-sys": "bundles DuckDB and its third-party libraries; each is listed under its own "
                     "license, marked \"Bundled in libduckdb-sys\"",
    "libsqlite3-sys": "bundles SQLite, listed under \"blessing\" (SQLite's public-domain dedication)",
    "aws-lc-sys": "bundles AWS-LC (a fork of BoringSSL and OpenSSL); its LICENSE covers the bundled code",
    "aws-lc-rs": None,  # `links` only; its code is Rust plus aws-lc-sys
    "rayon-core": None,  # `links` only as a uniqueness marker; no native code
    "core-foundation-sys": None,  # bindings to the macOS CoreFoundation framework
    "security-framework-sys": None,  # bindings to the macOS Security framework
}
# License files that live below a crate's root, inside bundled sources.
EXTRA_LICENSE_FILES = {
    "aws-lc-sys": ["aws-lc/third_party/fiat/LICENSE"],
}

# DuckDB as bundled in libduckdb-sys (duckdb.tar.gz). The tarball omits the
# upstream LICENSE files, so each component's license is taken from its source
# headers (checked by hand when the table was written; see `declared`), the
# copyright lines are read from its sources on every run, and the license text
# comes from scripts/licenses/. Every third_party directory and every DuckDB
# source with a license or copyright marker must be listed here.
DUCKDB_COMPONENTS = [
    dict(name="DuckDB", url="https://github.com/duckdb/duckdb", paths=[], declared="MIT",
         holders=["Copyright 2018-2026 Stichting DuckDB Foundation"],
         note="The database engine. Its copyright line is from DuckDB's LICENSE "
              "(github.com/duckdb/duckdb), which the crate's bundled sources omit."),
    dict(name="nanoarrow and the ADBC headers (Apache Arrow)", url="https://github.com/apache/arrow-nanoarrow",
         paths=["src/common/adbc/", "src/include/duckdb/common/adbc/",
                "src/include/duckdb/common/arrow/nanoarrow/"], declared="Apache-2.0"),
    dict(name="robin-hood-hashing hash constants (Martin Ankerl)", url="https://github.com/martinus/robin-hood-hashing",
         paths=["src/common/checksum.cpp", "src/common/types/hash.cpp"], declared="MIT"),
    dict(name="merge sort tree (salesforce.com)", url=None,
         paths=["src/include/duckdb/execution/merge_sort_tree.hpp"], declared="MIT"),
    dict(name="MD5 (Colin Plumb, via the SQLite test library)", url=None,
         paths=["src/common/crypto/md5.cpp"], declared="LicenseRef-Public-Domain",
         verbatim=[("src/common/crypto/md5.cpp", r"This code implements the MD5", r"do with it what you wish\.")]),
    dict(name="Brotli", url="https://github.com/google/brotli", paths=["third_party/brotli/"], declared="MIT"),
    dict(name="moodycamel::ConcurrentQueue", url="https://github.com/cameron314/concurrentqueue",
         paths=["third_party/concurrentqueue/"], declared="BSD-2-Clause AND Zlib",
         note="The lightweight semaphore is Jeff Preshing's, under the zlib License."),
    dict(name="fast_float", url="https://github.com/fastfloat/fast_float", paths=["third_party/fast_float/"],
         declared="MIT", note="By Daniel Lemire, João Paulo Magalhaes and contributors (per its source header)."),
    dict(name="FastPFor", url="https://github.com/lemire/FastPFor",
         paths=["third_party/fastpforlib/", "src/storage/compression/bitpacking.cpp"], declared="Apache-2.0"),
    dict(name="{fmt}", url="https://github.com/fmtlib/fmt", paths=["third_party/fmt/"], declared="MIT"),
    dict(name="FSST", url="https://github.com/cwida/fsst", paths=["third_party/fsst/"], declared="MIT"),
    dict(name="cpp-httplib", url="https://github.com/yhirose/cpp-httplib", paths=["third_party/httplib/"],
         declared="MIT"),
    dict(name="HyperLogLog (from Redis)", url="https://github.com/redis/redis", paths=["third_party/hyperloglog/"],
         declared="BSD-3-Clause"),
    dict(name="jaro_winkler", url="https://github.com/maxbachmann/jarowinkler-cpp",
         paths=["third_party/jaro_winkler/"], declared="MIT"),
    dict(name="libpg_query (PostgreSQL parser)", url="https://github.com/pganalyze/libpg_query",
         paths=["third_party/libpg_query/"], declared="PostgreSQL",
         drop=r"^Copyright \(C\) 1984, 1989",
         note="The grammar was generated by GNU Bison; its skeleton code is distributed under the "
              "Bison special exception, which lets it be used under the terms of the larger work."),
    dict(name="LZ4", url="https://github.com/lz4/lz4", paths=["third_party/lz4/"], declared="BSD-2-Clause"),
    dict(name="Mbed TLS", url="https://github.com/Mbed-TLS/mbedtls", paths=["third_party/mbedtls/"],
         declared="Apache-2.0 OR GPL-2.0-or-later"),
    dict(name="miniz", url="https://github.com/richgel999/miniz", paths=["third_party/miniz/"], declared="MIT"),
    dict(name="Apache Parquet format (Thrift-generated code)", url="https://github.com/apache/parquet-format",
         paths=["third_party/parquet/"], declared="Apache-2.0",
         note="Generated by the Thrift compiler from the Parquet format definition of the Apache "
              "Software Foundation; the generated files carry no notice of their own."),
    dict(name="PCG random", url="https://github.com/imneme/pcg-cpp", paths=["third_party/pcg/"],
         declared="Apache-2.0 OR MIT"),
    dict(name="pdqsort", url="https://github.com/orlp/pdqsort", paths=["third_party/pdqsort/"], declared="Zlib"),
    dict(name="RE2", url="https://github.com/google/re2", paths=["third_party/re2/"], declared="BSD-3-Clause",
         verbatim=[("third_party/re2/util/rune.cc", r"The authors of this software are Rob Pike",
                    r"FITNESS FOR ANY PARTICULAR PURPOSE\.")]),
    dict(name="ska_sort", url="https://github.com/skarupke/ska_sort", paths=["third_party/ska_sort/"],
         declared="BSL-1.0"),
    dict(name="SkipList", url="https://github.com/paulross/skiplist", paths=["third_party/skiplist/"],
         declared="MIT"),
    dict(name="Snappy", url="https://github.com/google/snappy", paths=["third_party/snappy/"],
         declared="BSD-3-Clause"),
    dict(name="t-digest (Derrick R. Burns)", url=None, paths=["third_party/tdigest/"], declared="Apache-2.0"),
    dict(name="Apache Thrift", url="https://github.com/apache/thrift", paths=["third_party/thrift/"],
         declared="Apache-2.0"),
    dict(name="utf8proc", url="https://github.com/JuliaStrings/utf8proc", paths=["third_party/utf8proc/"],
         declared="MIT AND Unicode-3.0",
         note="Contains data derived from the Unicode Character Database (Unicode License v3)."),
    dict(name="vergesort", url="https://github.com/Morwenn/vergesort", paths=["third_party/vergesort/"],
         declared="MIT"),
    dict(name="yyjson", url="https://github.com/ibireme/yyjson", paths=["third_party/yyjson/"], declared="MIT"),
    dict(name="Zstandard", url="https://github.com/facebook/zstd", paths=["third_party/zstd/"],
         declared="(BSD-3-Clause OR GPL-2.0-only) AND MIT",
         note="Includes libdivsufsort (Yuta Mori, MIT License)."),
]

INTRO = """\
Meridian: third-party software notices
======================================

Meridian includes software written by others. The app's core is a static
library built from the Rust crates and the C and C++ libraries listed below,
and their licenses ask that these notices go with every copy. Thank you to
everyone who wrote and maintains them.

This file is generated by scripts/third-party-notices.sh from core/Cargo.lock
for target {target}. Don't edit it by hand.

How to read it
  Part 1 lists every package under the license it declares, with the
  copyright lines from its license files (for bundled C and C++ code, from
  its source files). Where a license offers a choice, "Used under" names the
  option Meridian relies on. "Texts" points to license texts in Part 3;
  "Notices" points to files reproduced in Part 2.
  Part 2 reproduces NOTICE files, combined license files and license files
  with extra terms, word for word.
  Part 3 holds each distinct license text once.
"""


class Failure(Exception):
    pass


# ---------------------------------------------------------------- SPDX

def parse_spdx(expr):
    """Parses an SPDX expression (plus the legacy "/" separator) into a tree."""
    toks = re.findall(r"\(|\)|[^\s()]+", expr.replace("/", " OR "))
    pos = 0

    def peek():
        return toks[pos].upper() if pos < len(toks) else None

    def take():
        nonlocal pos
        if pos >= len(toks):
            raise ValueError("unexpected end")
        pos += 1
        return toks[pos - 1]

    def primary():
        t = take()
        if t == "(":
            node = disjunction()
            if take() != ")":
                raise ValueError("missing )")
            return node
        if t.upper() in ("AND", "OR", "WITH", ")"):
            raise ValueError("unexpected " + t)
        if peek() == "WITH":
            take()
            return ("with", t, take())
        return ("id", t)

    def conjunction():
        nodes = [primary()]
        while peek() == "AND":
            take()
            nodes.append(primary())
        return nodes[0] if len(nodes) == 1 else ("and", nodes)

    def disjunction():
        nodes = [conjunction()]
        while peek() == "OR":
            take()
            nodes.append(conjunction())
        return nodes[0] if len(nodes) == 1 else ("or", nodes)

    node = disjunction()
    if pos != len(toks):
        raise ValueError("trailing tokens")
    return node


def spdx_str(node, parent=None):
    """Canonical string: operands sorted, so "MIT OR Apache-2.0" == "Apache-2.0 OR MIT"."""
    kind = node[0]
    if kind == "id":
        return node[1]
    if kind == "with":
        return "%s WITH %s" % (node[1], node[2])
    parts = []
    for child in node[1]:
        if child[0] == kind:  # flatten A OR (B OR C)
            parts.extend(spdx_str(c, kind) for c in child[1])
        else:
            parts.append(spdx_str(child, kind))
    s = (" %s " % kind.upper()).join(sorted(set(parts)))
    return "(%s)" % s if parent == "and" and kind == "or" else s


def dnf(node):
    """Alternatives that satisfy the expression, each a frozenset of terms."""
    kind = node[0]
    if kind == "id":
        return [frozenset([node[1]])]
    if kind == "with":
        return [frozenset(["%s WITH %s" % (node[1], node[2])])]
    if kind == "or":
        return [alt for child in node[1] for alt in dnf(child)]
    alts = [frozenset()]
    for child in node[1]:
        alts = [a | b for a in alts for b in dnf(child)]
    return alts


def term_base(term):
    return term.split(" WITH ")[0]


def term_allowed(term, allowed):
    parts = term.split(" WITH ")
    return parts[0] in allowed and all(p in ALLOWED_EXCEPTIONS for p in parts[1:])


def rank(term):
    base = term_base(term)
    return PREFERENCE.index(base) if base in PREFERENCE else len(PREFERENCE)


def elect(expr):
    """Returns (terms used, uses weak copyleft) or None if no permissive choice exists."""
    alts = dnf(parse_spdx(expr))
    for allowed in (PERMISSIVE, PERMISSIVE | ACCEPTED_WEAK_COPYLEFT):
        ok = [a for a in alts if all(term_allowed(t, allowed) for t in a)]
        if ok:
            best = min(ok, key=lambda a: (len(a), sorted(rank(t) for t in a), sorted(a)))
            terms = sorted(best, key=lambda t: (rank(t), t))
            return terms, any(term_base(t) in ACCEPTED_WEAK_COPYLEFT for t in terms)
    return None


# ---------------------------------------------------------------- license text handling

def norm(s):
    for curly, straight in ((0x201C, '"'), (0x201D, '"'), (0x2018, "'"), (0x2019, "'")):
        s = s.replace(chr(curly), straight)
    s = s.replace("https://", "http://")
    s = re.sub(r"[`*#>_=~|-]+", " ", s)
    return re.sub(r"\s+", " ", s).strip().lower()


NOT_A_NOTICE = (
    "copyright notice", "copyright holder", "copyright owner", "copyright license",
    "copyright and permission", "copyright statement", "copyright, patent", "copyright and license",
    "copyrights in", "copyright and related", "copyright law", "copyright protection",
    "copyright and other", "copyright assignment",
)
PLACEHOLDER = re.compile(
    r"\[yyyy\]|\{yyyy\}|<year>|\[year\]|\{year\}|name of copyright owner|<copyright holders?>|"
    r"<owner>|\[fullname\]|<name of author>", re.I)


def strip_comment(line):
    s = line.strip()
    s = re.sub(r"^(?:/\*+|\*+/?|//+|#+|;+|!+|--)\s?", "", s).strip()
    return re.sub(r"\s*\*+/\s*$", "", s).strip()


def copyright_line(line):
    """Returns ("notice"|"placeholder", text) if the line is a copyright statement."""
    s = strip_comment(line)
    m = re.search(r"\b(?:MIT|BSD) License:?\s+(Copyright\b.*)$", s)
    if m:
        s = m.group(1)
    if not re.match(r"(?i)^(?:portions\s+)?(?:copyright\b|\(c\)\s|©)", s):
        return None
    if re.match(r"^\(c\)\s+(?:[a-z]|You\b)", s):  # a list item such as Apache-2.0 section 4(c)
        return None
    if PLACEHOLDER.search(s):
        return ("placeholder", s)
    low = s.lower()
    if any(p in low for p in NOT_A_NOTICE):
        return None
    if len(s) > 200:
        return None
    # "...Cameron Desrochers. Distributed under the terms of the simplified" -> keep the notice only.
    s = re.split(r"(?<=\.)\s+(?:Distributed|Licensed|Released) under\b|(?<=\.)\s+Use of this source", s)[0]
    return ("notice", re.sub(r"\s+", " ", s))


def classify(text):
    """The full license texts a file contains, as SPDX ids."""
    n = norm(text)
    found = set()
    if "apache license" in n and "terms and conditions for use, reproduction" in n:
        found.add("Apache-2.0 WITH LLVM-exception" if "llvm exceptions" in n else "Apache-2.0")
    if "permission is hereby granted, free of charge, to any person obtaining a copy" in n:
        found.add("MIT" if "the above copyright notice and this permission notice shall be included" in n
                  else "MIT-0")
    if "redistribution and use in source and binary forms" in n:
        found.add("BSD-3-Clause" if "neither the name" in n else "BSD-2-Clause")
    if re.search(r"permission to use, copy, modify, and(/or)? distribute this software for any purpose "
                 r"with or without fee is hereby granted", n):
        found.add("ISC" if "provided that the above copyright notice and this permission notice appear" in n
                  else "0BSD")
    if re.search(r"this software is provided ['\"]as is['\"]", n) and "permission is granted to anyone" in n:
        found.add("Zlib")
    if "unicode license v3" in n:
        found.add("Unicode-3.0")
    if "this is free and unencumbered software released into the public domain" in n:
        found.add("Unlicense")
    if "boost software license" in n and "permission is hereby granted, free of charge, to any person or organization" in n:
        found.add("BSL-1.0")
    if re.search(r"mozilla public license,? version 2\.0", n) and "1.1. \"contributor\"" in n:
        found.add("MPL-2.0")
    if "permission to use, copy, modify, and distribute this software and its documentation for any purpose, " \
       "without fee, and without a written agreement is hereby granted" in n:
        found.add("PostgreSQL")
    return found


TITLE = re.compile(r"(?i)^[#\s]*(the\s+)?(mit|isc|zlib|bsd[\w\s-]*?|0bsd|boost software)\s+licen[sc]e"
                   r"(\s*\(mit\))?(\s*,?\s*version 1\.0)?\s*:?\s*$")


def license_body(lic, text):
    """Splits a single-license file into (copyright lines, printable body, key).

    The key is None when the file carries more than the license itself (a
    preamble, extra terms), so it is reproduced word for word instead."""
    notices, kept = [], []
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        c = copyright_line(line)
        if c:
            # A notice whose parenthesis closes on the next line or two.
            notice = c[1]
            while (notice.count("(") > notice.count(")") and i < len(lines) and lines[i].strip()
                   and len(notice) < 300):
                notice += " " + lines[i].strip()
                i += 1
            if c[0] == "notice":
                notices.append(notice)
            continue
        if re.match(r"(?i)^\s*(all rights reserved\.?|spdx-license-identifier:.*)\s*$", line):
            continue
        kept.append(line)
    while kept and not kept[0].strip():
        kept.pop(0)
    if kept and TITLE.match(kept[0]) and lic != "Apache-2.0":
        kept.pop(0)
    body = "\n".join(kept).strip("\n")
    key = None
    if lic == "Apache-2.0":
        start = body.find("Apache License")
        end = body.find("END OF TERMS AND CONDITIONS")
        if start < 0 or end < 0:
            return notices, body, None
        head, tail = norm(body[:start]), letters(body[end + len("END OF TERMS AND CONDITIONS"):])
        # After the terms only the standard "how to apply" appendix (possibly
        # cut short) may follow; it is advice, not license terms, and is dropped.
        if head or not APACHE_APPENDIX.startswith(tail):
            return notices, body, None
        body = body[start:end + len("END OF TERMS AND CONDITIONS")]
        # Re-indent consistently: some copies indent the title block, some don't.
        title, terms = body.split("TERMS AND CONDITIONS", 1)
        lines = ("TERMS AND CONDITIONS" + terms).split("\n")
        indent = min((len(l) - len(l.lstrip()) for l in lines[1:] if l.strip()), default=0)
        title = [l.strip() for l in title.rstrip().split("\n")] + [""]
        body = "\n".join(title + [lines[0]] + [l[indent:] for l in lines[1:]])
        # The title block's URL varies between copies; the terms are what count.
        key = "apache license " + norm(body[body.find("TERMS AND CONDITIONS"):])
    key = key or norm(body)
    if not key.startswith(STARTS.get(lic, "")):
        return notices, body, None  # a preamble or other terms come first
    return notices, body, key


# How each license's own text begins, once the title and copyright lines are gone.
STARTS = {
    "MIT": "permission is hereby granted", "MIT-0": "permission is hereby granted",
    "BSD-2-Clause": "redistribution and use", "BSD-3-Clause": "redistribution and use",
    "ISC": "permission to use, copy, modify", "0BSD": "permission to use, copy, modify",
    "PostgreSQL": "permission to use, copy, modify", "Zlib": "this software is provided",
    "Unicode-3.0": "unicode license v3", "Unlicense": "this is free and unencumbered",
    "BSL-1.0": "boost software license", "MPL-2.0": "mozilla public license",
    "Apache-2.0": "apache license", "Apache-2.0 WITH LLVM-exception": "apache license",
}


def letters(s):
    return re.sub(r"[^a-z]", "", s.lower().replace("https://", "http://"))


# The standard appendix of the Apache License 2.0, letters only (set by main()
# from scripts/licenses/Apache-2.0.txt, without its placeholder copyright line).
APACHE_APPENDIX = ""


def read_text(path):
    with open(path, "rb") as f:
        data = f.read()
    return clean_text(data.decode("utf-8", errors="replace"))


def clean_text(text):
    text = text.lstrip(chr(0xFEFF)).replace("\r\n", "\n").replace("\r", "\n").expandtabs(4)
    return "\n".join(l.rstrip() for l in text.split("\n")).strip("\n")


def decomment(text):
    """Strips a comment prefix that (nearly) every line of a license file carries."""
    lines = text.split("\n")
    filled = [l.lstrip() for l in lines if l.strip()]
    for prefix in ("//", "#", "*", ";"):
        if filled and sum(1 for l in filled if l.startswith(prefix)) >= 0.8 * len(filled):
            out = []
            for line in lines:
                s = line.lstrip()
                if s.startswith(prefix):
                    s = s[len(prefix):]
                    out.append(s[1:] if s.startswith(" ") else s)
                else:
                    out.append(line)
            return clean_text("\n".join(out))
    return text


LICENSE_FILE = re.compile(r"(?i)^(licen[cs]es?|copying|copyright|notice|unlicense)"
                          r"([-_][\w.+-]+)?(\.(txt|md|markdown|rst))?$")


# ---------------------------------------------------------------- notices model

class Texts:
    """License texts (deduplicated by normalized content) and verbatim notices."""

    def __init__(self, root):
        self.root = root
        self.bodies = {}      # (license, key) -> dict(id, text, source)
        self.by_license = collections.defaultdict(int)
        self.notices = {}     # normalized text -> dict(id, text, files)
        self.canonical_cache = {}

    def body_id(self, lic, key, text, source):
        entry = self.bodies.get((lic, key))
        if entry is None:
            self.by_license[lic] += 1
            entry = dict(id="%s-%d" % (lic.replace(" WITH ", "+"), self.by_license[lic]), license=lic,
                         layouts=collections.Counter(), source=source)
            self.bodies[(lic, key)] = entry
        entry["layouts"][text] += 1
        return entry["id"]

    @staticmethod
    def body_text(entry):
        """The same words are laid out differently between copies; print the commonest layout."""
        return min(entry["layouts"].items(), key=lambda kv: (-kv[1], kv[0]))[0]

    def canonical(self, lic):
        """The standard text for a license from scripts/licenses/, registered as a body."""
        if lic in self.canonical_cache:
            return self.canonical_cache[lic]
        path = os.path.join(self.root, CANONICAL_DIR, lic + ".txt")
        if not os.path.exists(path):
            raise Failure("no license text for %s: add %s/%s.txt" % (lic, CANONICAL_DIR, lic))
        _, body, key = license_body(lic, read_text(path))
        if key is None:
            raise Failure("%s/%s.txt is not a plain %s text" % (CANONICAL_DIR, lic, lic))
        ident = self.body_id(lic, key, body, "%s/%s.txt" % (CANONICAL_DIR, lic))
        self.canonical_cache[lic] = ident
        return ident

    def notice_id(self, text, label):
        key = norm(text)
        entry = self.notices.get(key)
        if entry is None:
            entry = dict(id="N%d" % (len(self.notices) + 1), text=text, files=[])
            self.notices[key] = entry
        if label not in entry["files"]:
            entry["files"].append(label)
        return entry["id"]


class Entry:
    def __init__(self, kind, name, version, url, declared, where=None):
        self.kind = kind          # "crate" | "native" | "toolchain"
        self.name = name
        self.version = version
        self.url = url
        self.declared = declared
        self.where = where        # for native code: the crate and path it comes from
        self.text_ids = []
        self.notice_ids = []
        self.covered = set()      # license ids whose full text is present
        self.copyrights = []
        self.notes = []
        self.authors = []         # from Cargo.toml, named when a crate ships no license file
        self.fallback = []        # licenses whose standard text stands in for a missing file
        self.group = None
        self.used_under = None
        self.weak = False

    @property
    def label(self):
        return "%s %s" % (self.name, self.version) if self.version else self.name

    def add_copyrights(self, lines):
        for line in lines:
            if line not in self.copyrights:
                self.copyrights.append(line)


def resolve(entry, texts, failures):
    """Checks the license is acceptable and adds standard texts the package lacks."""
    try:
        node = parse_spdx(entry.declared)
    except ValueError:
        failures.append("%s: can't parse license %r" % (entry.label, entry.declared))
        return
    entry.group = spdx_str(node)
    choice = elect(entry.declared)
    if choice is None:
        failures.append("%s: %s is not permissive" % (entry.label, entry.declared))
        return
    terms, entry.weak = choice
    entry.used_under = " AND ".join(terms)
    for term in terms:
        base = term_base(term)
        if term in entry.covered or base in entry.covered or base in NO_TEXT_NEEDED:
            continue
        try:
            ident = texts.canonical(base)
        except Failure as e:
            failures.append("%s: %s" % (entry.label, e))
            continue
        if ident not in entry.text_ids:
            entry.text_ids.append(ident)
        entry.fallback.append(base)
        if entry.kind == "crate":
            note = "The crate has no %s license file; the standard text applies." % base
            if entry.authors:
                note += " Authors listed in its Cargo.toml: %s." % ", ".join(entry.authors)
            entry.notes.append(note)


# ---------------------------------------------------------------- collection

def run(cmd, cwd):
    try:
        return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout
    except FileNotFoundError:
        raise Failure("%s not found" % cmd[0])
    except subprocess.CalledProcessError as e:
        raise Failure("%s failed:\n%s" % (" ".join(cmd), e.stderr.strip()))


def crate_set(root, locked):
    manifest = os.path.join(root, "core", "Cargo.toml")
    lock = ["--locked"] if locked else []
    meta = json.loads(run(["cargo", "metadata", "--manifest-path", manifest, "--format-version", "1"] + lock, root))
    tree = run(["cargo", "tree", "--manifest-path", manifest, "-p", ROOT_CRATE, "--target", TARGET,
                "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}"] + lock, root)
    workspace = {p["name"] for p in meta["packages"] if p["id"] in set(meta["workspace_members"])}
    by_key = collections.defaultdict(list)
    for p in meta["packages"]:
        by_key[(p["name"], p["version"])].append(p)
    packages = {}
    for line in tree.splitlines():
        m = re.match(r"^(\S+) v(\S+)", line.strip())
        if not m or m.group(1) in workspace:
            continue
        found = by_key.get((m.group(1), m.group(2)), [])
        if len(found) != 1:
            raise Failure("can't match %s %s to one package in cargo metadata" % m.groups())
        packages[found[0]["id"]] = found[0]
    if not packages:
        raise Failure("cargo tree listed no dependencies for %s" % ROOT_CRATE)
    return sorted(packages.values(), key=lambda p: (p["name"].lower(), version_key(p["version"])))


def version_key(v):
    return [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"[.+-]", v)]


def collect_crate(p, texts, failures):
    url = p.get("repository") or p.get("homepage") or "https://crates.io/crates/%s/%s" % (p["name"], p["version"])
    entry = Entry("crate", p["name"], p["version"], url.rstrip("/"), p.get("license") or "")
    entry.authors = [re.sub(r"\s*<[^>]*>", "", a).strip() for a in p.get("authors") or []]
    src = os.path.dirname(p["manifest_path"])
    names = sorted(f for f in os.listdir(src) if LICENSE_FILE.match(f))
    paths = []
    for f in names:
        full = os.path.join(src, f)
        if os.path.isdir(full):
            paths.extend(os.path.join(full, g) for g in sorted(os.listdir(full)) if os.path.isfile(os.path.join(full, g)))
        else:
            paths.append(full)
    if p.get("license_file"):
        lf = os.path.normpath(os.path.join(src, p["license_file"]))
        if lf not in paths:
            paths.append(lf)
    for extra in EXTRA_LICENSE_FILES.get(p["name"], []):
        full = os.path.join(src, extra)
        if not os.path.isfile(full):
            raise Failure("%s: expected bundled license file %s is missing" % (entry.label, extra))
        paths.append(full)
    for path in paths:
        rel = os.path.relpath(path, src)
        add_text(entry, decomment(read_text(path)), "%s/%s" % (entry.label, rel), texts)
    if not entry.declared:
        failures.append("%s: no license in Cargo.toml" % entry.label)
    if p["name"] in NATIVE_REVIEWED:
        if NATIVE_REVIEWED[p["name"]]:
            entry.notes.append("This crate " + NATIVE_REVIEWED[p["name"]] + ".")
    elif p["name"].endswith("-sys") or p.get("links"):
        failures.append("%s: native crate not reviewed; check its bundled sources and add it to "
                        "NATIVE_REVIEWED" % entry.label)
    return entry


def add_text(entry, text, label, texts):
    if not text.strip():
        return
    found = classify(text)
    entry.covered |= found
    if len(found) == 1:
        lic = next(iter(found))
        notices, body, key = license_body(lic, text)
        if key is not None:
            entry.add_copyrights(notices)
            ident = texts.body_id(lic, key, body, label)
            if ident not in entry.text_ids:
                entry.text_ids.append(ident)
            return
    ident = texts.notice_id(text, label)
    if ident not in entry.notice_ids:
        entry.notice_ids.append(ident)


def comment_block(text, start, end, label):
    m = re.search(start + r".*?" + end, text, re.S)
    if not m:
        raise Failure("%s: notice /%s/ not found; update the component table" % (label, start))
    return clean_text("\n".join(strip_comment(l) for l in m.group(0).split("\n")))


def collect_duckdb(sys_pkg, texts, failures):
    tar_path = os.path.join(os.path.dirname(sys_pkg["manifest_path"]), "duckdb.tar.gz")
    where = "libduckdb-sys %s" % sys_pkg["version"]
    files = {}
    with tarfile.open(tar_path) as tar:
        for m in tar.getmembers():
            if m.isfile() and m.name.startswith("duckdb/"):
                files[m.name[len("duckdb/"):]] = tar.extractfile(m).read().decode("utf-8", errors="replace")

    def owner(path):
        for c in DUCKDB_COMPONENTS:
            if any(path == p or (p.endswith("/") and path.startswith(p)) for p in c["paths"]):
                return c
        return None

    # Every third_party directory and every marked DuckDB source must be listed.
    unlisted = set()
    marker = re.compile(r"(?i)copyright|licen[sc]ed under|apache license|mit license|public domain|"
                        r"spdx-license-identifier|permission is hereby granted|redistribution and use")
    for path, text in files.items():
        if owner(path):
            continue
        if path.startswith("third_party/"):
            unlisted.add("third_party/" + path.split("/")[1])
        elif marker.search(text):
            unlisted.add(path)
    for path in sorted(unlisted):
        failures.append("%s: DuckDB source %s has its own license notice; add it to DUCKDB_COMPONENTS"
                        % (where, path))
    entries = []
    for c in DUCKDB_COMPONENTS:
        e = Entry("native", c["name"], None, c["url"], c["declared"],
                  where="%s (%s)" % (where, ", ".join("duckdb/" + p.rstrip("/") for p in c["paths"])
                                     or "duckdb/src"))
        mine = sorted(p for p in files if owner(p) is c)
        if c["paths"] and not mine:
            failures.append("%s: no sources found for %s; update DUCKDB_COMPONENTS" % (where, c["name"]))
        lines = set(c.get("holders", []))
        drop = re.compile(c["drop"]) if c.get("drop") else None
        for path in mine:
            for line in files[path].split("\n"):
                hit = copyright_line(line)
                if hit and hit[0] == "notice" and not (drop and drop.search(hit[1])):
                    lines.add(hit[1].rstrip(" ,"))
            asf = re.search(r"Licensed to (.{3,80}?) under one\W+or\W+more\W+contributor\W+license\W+agreements",
                            files[path], re.S)
            if asf:
                lines.add("Licensed to %s under one or more contributor license agreements."
                          % re.sub(r"\s+", " ", asf.group(1)))
        e.add_copyrights(sorted(lines))
        for path, start, end in c.get("verbatim", []):
            if path not in files:
                failures.append("%s: %s is missing; update DUCKDB_COMPONENTS" % (where, path))
                continue
            e.notice_ids.append(texts.notice_id(comment_block(files[path], start, end, path),
                                                "%s: duckdb/%s" % (where, path)))
        if c.get("note"):
            e.notes.append(c["note"])
        entries.append(e)
    return entries


def collect_sqlite(sys_pkg, texts):
    path = os.path.join(os.path.dirname(sys_pkg["manifest_path"]), "sqlite3", "sqlite3.c")
    where = "libsqlite3-sys %s" % sys_pkg["version"]
    text = read_text(path)
    m = re.search(r'#define SQLITE_VERSION\s+"([\d.]+)"', text)
    version = m.group(1) if m else None
    e = Entry("native", "SQLite", version, "https://sqlite.org", "blessing", where="%s (sqlite3/sqlite3.c)" % where)
    block = comment_block(text, r"The author disclaims copyright", r"never taking more than you give\.", path)
    e.notice_ids.append(texts.notice_id(block, "%s: sqlite3/sqlite3.c" % where))
    e.notes.append("SQLite is in the public domain.")
    return e


def rust_std():
    e = Entry("toolchain", "Rust standard library", None, "https://github.com/rust-lang/rust", "Apache-2.0 OR MIT")
    e.notes.append("Linked into every Rust program. Copyrights are retained by the Rust Project Developers "
                   "(https://thanks.rust-lang.org). Notices for the crates the standard library itself uses "
                   "ship with the Rust toolchain in share/doc/rust/COPYRIGHT-library.html.")
    return e


# ---------------------------------------------------------------- output

RULE = "=" * 78
THIN = "-" * 78


def render(entries, texts, counts):
    out = [INTRO.format(target=TARGET)]
    crates = sum(1 for e in entries if e.kind == "crate")
    native = sum(1 for e in entries if e.kind == "native")
    out.append("Summary\n  %d Rust crates, %d bundled C and C++ components, and the Rust standard library.\n"
               % (crates, native))
    out.append(summary_table(counts))
    out.append("")

    groups = collections.defaultdict(list)
    for e in entries:
        groups[e.group].append(e)
    out += [RULE, "Part 1. Packages by license", RULE, ""]
    for group, members in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        out += [THIN, wrap("%s (%d %s)" % (group, len(members), "package" if len(members) == 1 else "packages"),
                           "    ")]
        if " OR " in group:
            used = sorted({e.used_under for e in members})
            out.append(wrap("Used under: " + " / ".join(used), "    "))
        out += [THIN, ""]
        if any(e.weak for e in members):
            out.append("These packages are used unmodified. Their source code is available from the\n"
                       "repositories listed with them (Mozilla Public License 2.0, section 3.2(a)).\n"
                       "Meridian's Swift bindings are generated by UniFFI's uniffi_bindgen, from the\n"
                       "same repository and under the same license.\n")
        order = {"crate": 0, "native": 1, "toolchain": 2}
        for e in sorted(members, key=lambda e: (order[e.kind], e.name.lower(), version_key(e.version or ""))):
            out.append(e.label + ("  " + e.url if e.url else ""))
            if e.where:
                out.append("    Bundled in %s" % e.where)
            refs = []
            if e.text_ids:
                refs.append("Texts: " + ", ".join(e.text_ids))
            if e.notice_ids:
                refs.append("Notices: " + ", ".join(e.notice_ids))
            if refs:
                out.append("    " + "; ".join(refs))
            for line in e.copyrights:
                out.append("    " + line)
            for note in e.notes:
                out.append(wrap("    Note: " + note, "          "))
            out.append("")

    out += [RULE, "Part 2. Notices reproduced word for word", RULE, ""]
    for entry in sorted(texts.notices.values(), key=lambda n: int(n["id"][1:])):
        out += [THIN, "[%s] %s" % (entry["id"], entry["files"][0])]
        if len(entry["files"]) > 1:
            out.append(wrap("     Same text in: " + ", ".join(entry["files"][1:]), "     "))
        out += [THIN, "", entry["text"], ""]

    out += [RULE, "Part 3. License texts", RULE, ""]
    for entry in sorted(texts.bodies.values(), key=lambda b: (PREFERENCE.index(term_base(b["license"]))
                                                             if term_base(b["license"]) in PREFERENCE else 99,
                                                             b["license"], int(b["id"].rsplit("-", 1)[1]))):
        out += [THIN, "[%s] %s" % (entry["id"], LICENSE_NAMES.get(entry["license"], entry["license"])), THIN,
                "", Texts.body_text(entry), ""]
    return "\n".join(out).rstrip("\n") + "\n"


def summary_table(counts):
    """Package counts per declared license."""
    return "\n".join(wrap("  %5d  %s" % (n, group), " " * 11)
                     for group, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])))


def wrap(text, indent, width=78):
    lines = []
    line = " " * (len(text) - len(text.lstrip(" ")))
    words = text.strip().split(" ")
    for w in words:
        if len(line) + len(w) + 1 > width and line.strip():
            lines.append(line.rstrip())
            line = indent
        line += ("" if line.endswith(" ") or not line.strip() else " ") + w
    lines.append(line.rstrip())
    return "\n".join(lines)


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", required=True, help="repository root")
    ap.add_argument("--check", action="store_true", help="fail if the committed notices are stale")
    args = ap.parse_args()
    root = os.path.abspath(args.root)
    out_path = os.path.join(root, OUTPUT)

    global APACHE_APPENDIX
    apache = read_text(os.path.join(root, CANONICAL_DIR, "Apache-2.0.txt"))
    apache = "\n".join(l for l in apache.split("\n") if not copyright_line(l))
    APACHE_APPENDIX = letters(apache.split("END OF TERMS AND CONDITIONS", 1)[1])

    texts = Texts(root)
    failures = []
    packages = crate_set(root, locked=args.check)
    entries = []
    for p in packages:
        entries.append(collect_crate(p, texts, failures))
    by_name = {p["name"]: p for p in packages}
    if "libduckdb-sys" in by_name:
        entries += collect_duckdb(by_name["libduckdb-sys"], texts, failures)
    if "libsqlite3-sys" in by_name:
        entries.append(collect_sqlite(by_name["libsqlite3-sys"], texts))
    entries.append(rust_std())
    for e in entries:
        resolve(e, texts, failures)
    if failures:
        print("third-party notices: these packages need attention (licenses must be permissive; "
              "see CLAUDE.md):", file=sys.stderr)
        for f in failures:
            print("  " + f, file=sys.stderr)
        return 1

    counts = collections.Counter(e.group for e in entries)
    text = render(entries, texts, counts)

    if args.check:
        current = ""
        if os.path.exists(out_path):
            with open(out_path, encoding="utf-8") as f:
                current = f.read()
        if current != text:
            print("%s is stale: run scripts/third-party-notices.sh and commit the result" % OUTPUT, file=sys.stderr)
            return 1
        print("third-party notices: up to date")
        return 0

    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(text)
    crates = sum(1 for e in entries if e.kind == "crate")
    native = sum(1 for e in entries if e.kind == "native")
    print("third-party notices: %d Rust crates, %d bundled C/C++ components, Rust standard library"
          % (crates, native))
    print(summary_table(counts))
    weak = sorted(e.label for e in entries if e.weak)
    if weak:
        print("note: MPL-2.0 (file-level copyleft, used unmodified): " + ", ".join(weak))
    fallback = sorted(e.label for e in entries if e.kind == "crate" and e.fallback)
    if fallback:
        print("note: no license file in the crate, standard text used: " + ", ".join(fallback))
    print("wrote %s (%d KB)" % (OUTPUT, (len(text.encode("utf-8")) + 1023) // 1024))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Failure as e:
        print("third-party notices: " + str(e), file=sys.stderr)
        sys.exit(1)
