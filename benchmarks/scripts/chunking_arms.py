"""Crawl4AI's chunkers over markdown files. Runs inside .work/venv-crawl4ai.

usage: chunking_arms.py <out.jsonl> <size,size,...> <source>=<md dir> [<source>=<md dir> ...]
Each output row: source, strategy, size, id, seconds, chunks.
Window sizes are words: the target token size divided by Crawl4AI's own WORD_TOKEN_RATE (1.3), the
ratio its LLM extraction uses to turn a token budget into words. Overlap is its OVERLAP_RATE (10%).
"""
import json, sys, time
from pathlib import Path

from crawl4ai.chunking_strategy import OverlappingWindowChunking, RegexChunking, SlidingWindowChunking
from crawl4ai.config import OVERLAP_RATE, WORD_TOKEN_RATE
from crawl4ai.utils import merge_chunks


def strategies(size):
    words = round(size / WORD_TOKEN_RATE)
    return {
        "sliding window": SlidingWindowChunking(window_size=words, step=words // 2).chunk,
        "overlapping window": OverlappingWindowChunking(window_size=words, overlap=int(words * OVERLAP_RATE)).chunk,
        "regex": RegexChunking().chunk,
        "regex + merge": lambda md: merge_chunks(RegexChunking().chunk(md), target_size=size,
                                                 overlap=int(size * OVERLAP_RATE), word_token_ratio=WORD_TOKEN_RATE),
    }


def main(out, sizes, sources):
    with open(out, "a") as f:
        for spec in sources:
            source, folder = spec.split("=", 1)
            for path in sorted(Path(folder).glob("*.md")):
                md = path.read_text()
                for size in map(int, sizes.split(",")):
                    for name, fn in strategies(size).items():
                        t0 = time.perf_counter()
                        chunks = [c for c in fn(md) if c.strip()]
                        secs = time.perf_counter() - t0
                        f.write(json.dumps({"source": source, "strategy": name, "size": size, "id": path.stem,
                                            "seconds": secs, "chunks": chunks}) + "\n")


main(sys.argv[1], sys.argv[2], sys.argv[3:])
