#!/usr/bin/env python3
"""Fills a folder with realistic test data for a sync test account.

    scripts/test-data.py TARGET --batch N [--wiki K]

Each batch adds about N files under TARGET/davka-N (same seed, same files):
documents by year and month, small images, a deep "projects" tree, a few
larger files and the awkward cases (diacritics, NFD names from a Mac,
spaces, emoji, long names, an empty file). With --wiki K it also saves K
random pages of the Czech Wikipedia (HTML), slowly, as an anonymous
client. Only for a test account: everything goes to the server.
"""
import argparse
import os
import random
import time
import unicodedata
import urllib.request
import zlib
import struct

WORDS = (
    "synchronizace soubor složka iCloud zálohování dokument fotografie projekt "
    "výkaz faktura smlouva zápis porada rozpočet plán report návrh analýza "
    "měření teplota hodnota tabulka graf příloha kapitola úvod závěr shrnutí "
    "Praha Brno Ostrava léto zima jaro podzim rodina dovolená výlet hory moře"
).split()


def text(rng, paragraphs):
    out = []
    for _ in range(paragraphs):
        words = [rng.choice(WORDS) for _ in range(rng.randint(30, 120))]
        out.append(" ".join(words).capitalize() + ".")
    return "\n\n".join(out) + "\n"


def png(rng, size):
    """A small valid PNG with random colour blocks."""
    w = h = size
    c = [rng.randrange(256) for _ in range(6)]
    raw = b"".join(
        b"\0" + b"".join(bytes(c[0:3] if ((x // 8) + (y // 8)) % 2 else c[3:6]) for x in range(w))
        for y in range(h)
    )

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode) as f:
        f.write(data)


def batch(root, n, seed):
    rng = random.Random(seed)
    count = 0
    months = ["leden", "únor", "březen", "duben", "květen", "červen",
              "červenec", "srpen", "září", "říjen", "listopad", "prosinec"]
    # Documents: about 45 %.
    for i in range(n * 45 // 100):
        year = rng.choice([2021, 2022, 2023, 2024, 2025])
        month = rng.choice(months)
        kind = rng.choice(["zapis", "faktura", "poznamky", "report", "navrh"])
        ext = rng.choice([".txt", ".md", ".csv"])
        body = text(rng, rng.randint(1, 6))
        if ext == ".csv":
            body = "datum;hodnota;poznámka\n" + "".join(
                f"{year}-{rng.randint(1,12):02}-{rng.randint(1,28):02};{rng.randint(0,9999)};{rng.choice(WORDS)}\n"
                for _ in range(rng.randint(5, 200)))
        write(os.path.join(root, "dokumenty", str(year), month, f"{kind}-{i:04}{ext}"), body)
        count += 1
    # Images: about 20 %.
    for i in range(n * 20 // 100):
        album = rng.choice(["dovolena", "rodina", "hory", "zahrada", "auto"])
        write(os.path.join(root, "fotky", album, f"IMG_{seed:02}{i:04}.png"), png(rng, rng.choice([16, 32, 64])))
        count += 1
    # Projects: deep trees, about 30 %.
    for i in range(n * 30 // 100):
        depth = rng.randint(2, 7)
        parts = [f"projekt-{rng.randint(1, 8)}"] + [rng.choice(["src", "lib", "docs", "data", "test", "modul", "v2"]) for _ in range(depth)]
        ext = rng.choice([".rs", ".py", ".json", ".txt", ".html"])
        write(os.path.join(root, "projekty", *parts, f"soubor_{i:05}{ext}"), text(rng, rng.randint(1, 3)))
        count += 1
    # Awkward names.
    awkward = [
        "Příloha – smlouva (finální) v2.txt",
        unicodedata.normalize("NFD", "Žluťoučký kůň z Macu.txt"),
        "soubor s   mezerami .txt",
        "emoji 🎷 saxofon.txt",
        "dlouhé jméno " + "abcdefghij" * 18 + ".txt",
        "prázdný soubor.txt",
        ".skrytý soubor",
        "tečka.na.konci.",
        "UPPER and lower.TXT",
    ]
    for name in awkward:
        body = "" if name.startswith("prázdný") else f"{name}\n"
        write(os.path.join(root, "zaludne nazvy", name), body)
        count += 1
    # A few larger files.
    for size_mb in (1, 5, 12):
        write(os.path.join(root, "velke", f"data-{size_mb}MB.bin"), rng.randbytes(size_mb * 1024 * 1024))
        count += 1
    return count


def wiki(root, k):
    os.makedirs(root, exist_ok=True)
    done = 0
    for _ in range(k):
        req = urllib.request.Request(
            "https://cs.wikipedia.org/api/rest_v1/page/random/html",
            headers={"User-Agent": "ikeloud-test-data/0.1 (sync client test)"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                html = r.read()
                title = r.url.rsplit("/", 1)[-1]
        except Exception as e:  # keep going: test data, not a crawler
            print("wiki:", e)
            time.sleep(5)
            continue
        title = urllib.request.unquote(title).replace("/", "-")[:120] or f"stranka-{done}"
        write(os.path.join(root, f"{title}.html"), html)
        done += 1
        time.sleep(1.0)  # politely, one page a second
    return done


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("target")
    ap.add_argument("--batch", type=int, required=True, help="batch number (also the seed)")
    ap.add_argument("--files", type=int, default=1500)
    ap.add_argument("--wiki", type=int, default=0)
    a = ap.parse_args()
    root = os.path.join(a.target, f"davka-{a.batch}")
    # Generated in a sibling folder first, then moved in at once: the sync
    # sees whole files, never ones still being written.
    # (~/.cache: outside the synced folder, on the same filesystem as home.)
    staging = os.path.expanduser(f"~/.cache/ikeloud-staging-{a.batch}")
    n = batch(staging, a.files, a.batch)
    if a.wiki:
        n += wiki(os.path.join(staging, "wikipedie"), a.wiki)
    os.makedirs(a.target, exist_ok=True)
    os.rename(staging, root)
    print(f"{n} files in {root}")


main()
