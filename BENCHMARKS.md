# Benchmarks

`benches/fill.rs` times nine fills. Each case parses a template and fills it once per run. The bench reports the median and fastest run. It runs each case at least 5 times and keeps going for up to 2 seconds, with at most 50 runs.

| Case | What it fills |
| --- | --- |
| `tags/split-1k` | 1,000 tags, each split across three runs |
| `loop/table-rows-20k` | A table row loop over 20,000 items of five cells |
| `loop/nested-200x100` | Paragraph loops nested 200 by 100 |
| `conditions/5k` | 5,000 inline sections, half shown |
| `expressions/loop-5k` | A 5,000 item loop of arithmetic, comparisons, paths and `$index` |
| `html/blocks-300k` | About 300 KB of HTML: paragraphs, links, lists and a 200 row table |
| `html/style-sheet-3k` | 3,000 HTML elements against a 60 rule style sheet |
| `errors/2k` | A template with 2,000 errors |
| `package/report-2k` | A package with a 2,000 row loop, HTML, a header, a footer and core properties |

## Running

Run the bench in the fixed container so that results can be compared:

```sh
benches/run.sh                                    # print timings
benches/run.sh --compare benches/baseline.json    # fail if a case is over 15% slower
benches/run.sh --compare benches/baseline.json --threshold 10
benches/run.sh --save benches/baseline.json       # record a new baseline
benches/run.sh html                               # only cases whose name contains "html"
```

The container uses a pinned `rust:1.95-bookworm` image and is limited to 2 cores (`--cpuset-cpus 0-1 --cpus 2`) and 4 GB of memory with no swap. The script mounts the repository, so Docker must be able to see its folder. Docker Desktop shares `/Users` by default, but not `/tmp`. `cargo bench --all-features --bench fill` runs the same cases outside the container.

## Baseline

`benches/baseline.json` was recorded at `560a3cd` on an Apple Silicon Mac with Docker Desktop (arm64):

| Case | Median ms |
| --- | ---: |
| `tags/split-1k` | 3.09 |
| `loop/table-rows-20k` | 56.79 |
| `loop/nested-200x100` | 8.20 |
| `conditions/5k` | 10.72 |
| `expressions/loop-5k` | 3.38 |
| `html/blocks-300k` | 20.40 |
| `html/style-sheet-3k` | 22.74 |
| `errors/2k` | 1.83 |
| `package/report-2k` | 1.28 |

Two runs on the same machine differed by less than 3% per case. The exception is `expressions/loop-5k`, which differed by 8% because its time is short. Timings depend on the host, so compare only against a baseline taken on the same machine. Record a new baseline before comparing on a different machine.

`conditions/5k` grows linearly: 1,250, 2,500 and 5,000 sections take 3.4, 6.2 and 11 ms. Before `560a3cd` each loop scanned back to the document start for an enclosing cell, and the same sections took 41, 181 and 668 ms.

## Tracking regressions

1. Before a change, run `benches/run.sh --save benches/local.json` on main. Paths are inside the repository, which the container mounts, and `benches/local.json` is ignored by git. The committed baseline also works if it came from the same machine.
2. After the change, run `benches/run.sh --compare benches/local.json`.
3. If a change makes the bench faster or slower on purpose, rerun with `--save benches/baseline.json` and commit the new baseline with the change.
