# Memory evidence

## Historical engine decision input

The rows below transcribe `memory-summary-v1.json` exactly before removal of that raw result. They contain historical engine-selection evidence and do not match the current inputs. They do not pass the current validator because the run did not retain dependency and build-activity receipts. All byte and nanosecond values are raw. Ranges include the stated minimum and maximum.

| Scenario | Candidate | Reps | Peak bytes min / median / max | Startup ns min / median / max | Process count min / median / max | Sample window ns min / max | Free memory % min / max |
|---|---:|---:|---:|---:|---:|---:|---:|
| loaded-single-12 | cef | 3 | 1117627592 / 1126934064 / 1143776744 | 3361261500 / 3470958750 / 5111487875 | 19 / 19 / 19 | 10003613792 / 10010904375 | 47 / 52 |
| loaded-single-12 | qt | 3 | 1875759528 / 1899352488 / 1909936384 | 4295933208 / 4450687334 / 4788925084 | 15 / 15 / 15 | 10010488625 / 10010539166 | 50 / 51 |
| loaded-single-12 | webkit | 3 | 473593416 / 482309800 / 485078744 | 2609942083 / 2691958792 / 2812676292 | 17 / 17 / 17 | 10007020625 / 10010405084 | 50 / 52 |
| loaded-single-20 | cef | 3 | 1547686224 / 1562365904 / 1586057240 | 2484890959 / 3845262584 / 4174916791 | 27 / 27 / 27 | 10005484000 / 14570125000 | 45 / 52 |
| loaded-single-20 | qt | 3 | 2801384768 / 2829106256 / 2881240408 | 3292120542 / 7109888750 / 7234325083 | 23 / 23 / 23 | 10005384125 / 12819668583 | 41 / 49 |
| loaded-single-20 | webkit | 3 | 651776184 / 673059120 / 694751464 | 3220615708 / 4035876542 / 5845966042 | 25 / 25 / 25 | 10003867833 / 10010392916 | 38 / 52 |
| loaded-single-7 | cef | 3 | 790382952 / 796346680 / 799214120 | 2515644958 / 2601474417 / 4031047708 | 14 / 14 / 14 | 10001330334 / 10004488791 | 46 / 51 |
| loaded-single-7 | qt | 3 | 1069954560 / 1127577016 / 1139832368 | 2937996917 / 3305886875 / 3886609875 | 10 / 10 / 10 | 10002902916 / 10008794125 | 49 / 52 |
| loaded-single-7 | webkit | 3 | 339863032 / 353314344 / 366126632 | 1928479500 / 1991106708 / 2183581417 | 12 / 12 / 12 | 10007658042 / 10010666750 | 46 / 51 |
| loaded-split-7 | cef | 3 | 780028120 / 795019408 / 796216160 | 1141212667 / 1463493583 / 1516101958 | 14 / 14 / 14 | 10005275916 / 10010242416 | 43 / 53 |
| loaded-split-7 | qt | 3 | 1153267344 / 1154921936 / 1259779680 | 1042922208 / 1375981083 / 1382423000 | 10 / 10 / 10 | 10000890125 / 10005326833 | 44 / 55 |
| loaded-split-7 | webkit | 3 | 342107664 / 350807520 / 359622208 | 1088464833 / 1089277750 / 1159658083 | 12 / 12 / 12 | 10000503459 / 10002812292 | 42 / 53 |
| unloaded-single-20 | cef | 3 | 439904456 / 440527192 / 444770480 | 2039602417 / 2093529708 / 2500303209 | 8 / 8 / 8 | 10003532916 / 10005231834 | 44 / 49 |
| unloaded-single-20 | qt | 3 | 712119960 / 717116936 / 723474000 | 2651937166 / 2743831125 / 2968180416 | 4 / 4 / 4 | 10005092584 / 10009096416 | 45 / 50 |
| unloaded-single-20 | webkit | 3 | 645779424 / 662097888 / 675729472 | 1697732333 / 1860561709 / 1893867750 | 25 / 25 / 25 | 10000617959 / 10005157500 | 42 / 51 |

The WebKit unloaded result did not prove native view release and did not reduce memory. The current product policy therefore keeps only one live `WKWebView`; inactive tabs are metadata only.

## Historical WebKit media cycle

| Item | Historical value |
|---|---:|
| Hardware | Apple M4, 17179869184 bytes memory |
| Production ceiling | 2000000000 bytes |
| Four live views peak | 3603854752 bytes |
| Four live views passed | no |
| Four live views background views paused and muted | yes |
| Four live views reclaimed below ceiling | no |
| One-live cycle tab records / recent slots / live slots | 20 / 4 / 1 |
| One-live cycle profile mode | off-the-record |
| Observations per stage | 11 |
| One-live stage peaks | 1102536048, 1162714792, 1135812624, 1289757144 bytes |
| One-live maximum peak | 1289757144 bytes |
| Owned reference releases / native releases / exact captures | 4 / 4 / 4 |
| Build contamination / inputs verified / service baseline restored | no / yes / yes |
| One-live cycle passed at run | yes |
| Chrome reference | 153.0.8010.37, isolated-temporary, incomplete peak 830501200 bytes |
| Chrome reference completeness / reason / blocking | no / process membership changed during measurement / no |

The historical media run predates the final validator and source hardening, so it is not a current regression baseline.

## Current Objective-C benchmark

The Objective-C benchmark completed all 30 required phase repetitions. Each value below is the minimum, median, and maximum peak across three repetitions.

| Tabs and phase | Peak bytes min / median / max |
|---|---:|
| 7 loaded | 131341296 / 132012992 / 132979744 |
| 7 one-live | 59657024 / 60083008 / 60558144 |
| 7 reopened | 72518416 / 72747816 / 72862504 |
| 12 loaded | 202418592 / 203434640 / 214575712 |
| 12 one-live | 66538328 / 67111744 / 69176200 |
| 12 reopened | 79072088 / 79416128 / 79661864 |
| 20 loaded | 315638032 / 317112568 / 338067656 |
| 20 one-live | 69127024 / 70568792 / 74009504 |
| 20 reopened | 81742632 / 83856168 / 84921152 |
| 7 split stress | 161684560 / 161700920 / 162405456 |

The highest production-policy peak was 84921152 bytes. It stayed below the 2000000000-byte limit.

`Web --memory-benchmark` creates deterministic in-memory WebKit content. For each of 7, 12, and 20 tab records it measures loaded, one-live, and reopened phases; it also runs a benchmark-only seven-tab split stress case. Every accepted phase has three repetitions and eleven observations spanning at least ten seconds.

It captures the same WebKit service baseline before the first case. It excludes only those pre-existing identities. It recomputes the members for every observation. Active same-application services therefore remain attributed later in the run.

It records `proc_pid_rusage` identities and counters. It rejects changed membership and reused process identifiers. It verifies final receipt members against the run-wide baseline. It obtains each total with `/usr/bin/footprint -j`. A weak reference proves that each unloaded native view released.
