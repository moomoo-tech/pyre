# Benchmark Summary

Generated: 2026-03-24T16:35:41.355462


## Basic Throughput

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Hello World | 219,857 (1.0ms) | 82,599 (3.9ms) | 59,374 (0.9ms) | 92,868 (35.5ms) |
| JSON small (3 fields) | 221,563 (0.9ms) | 67,753 (4.7ms) | 203,033 (1.0ms) | 91,099 (29.5ms) |
| JSON medium (100 users) | 70,339 (3.7ms) | 13,343 (21.8ms) | 2,652 (4.7ms) | 45,166 (23.7ms) |
| JSON large (500 records) | 5,431 (46.9ms) | 1,998 (150.2ms) | 5,279 (48.2ms) | 4,488 (85.3ms) |

## CPU Intensive

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| fib(10) | 215,296 (0.9ms) | 60,859 (5.3ms) | 213,168 (1.1ms) | 79,297 (26.2ms) |
| fib(20) | 11,561 (22.0ms) | 1,981 (151.3ms) | 11,073 (23.1ms) | 10,586 (41.7ms) |
| fib(30) | 96 (1220.0ms) | 2 (0.0ms) | 91 (1240.0ms) | 75 (989.0ms) |

## Python Ecosystem

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Pure Python sum(10k) | 81,155 (3.1ms) | 18,968 (14.9ms) | 79,931 (3.2ms) | 43,863 (23.1ms) |
| numpy mean(10k) | 8,260 (30.9ms) | 8,784 (29.1ms) | 32,820 (22.8ms) |
| numpy SVD 100x100 | 4,151 (61.2ms) | 4,173 (61.0ms) | 5,471 (52.0ms) |

## I/O Simulation

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| sleep(1ms) | 7,813 (32.4ms) | 53,399 (4.9ms) | 7,887 (32.3ms) | 93,184 (3.9ms) |

## JSON Parsing

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Parse 41B JSON | 219,278 (0.9ms) | 70,998 (4.4ms) | 217,354 (0.9ms) | 82,883 (27.6ms) |
| Parse 7KB JSON | 98,578 (2.5ms) | 21,148 (13.3ms) | 98,938 (2.5ms) | 54,148 (25.0ms) |
| Parse 93KB JSON | 10,657 (23.9ms) | 1,925 (155.2ms) | 10,299 (24.8ms) | 8,314 (56.0ms) |