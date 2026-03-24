# Benchmark Summary

Generated: 2026-03-24T17:34:39.543970


## Basic Throughput

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Hello World | 217,901 (0.9ms) | 75,753 (4.3ms) | 219,879 (0.9ms) | 87,497 (34.8ms) |
| JSON small (3 fields) | 218,609 (0.9ms) | 74,036 (4.5ms) | 219,752 (0.9ms) | 85,850 (32.7ms) |
| JSON medium (100 users) | 62,871 (4.2ms) | 13,773 (21.2ms) | 69,057 (3.7ms) | 44,405 (26.4ms) |
| JSON large (500 records) | 5,053 (50.4ms) | 1,994 (154.6ms) | 5,250 (48.5ms) | 4,908 (65.7ms) |

## CPU Intensive

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| fib(10) | 206,549 (1.0ms) | 63,343 (5.1ms) | 212,099 (1.0ms) | 80,732 (29.5ms) |
| fib(20) | 10,719 (24.2ms) | 1,855 (169.8ms) | 11,350 (22.5ms) | 11,281 (37.9ms) |
| fib(30) | 87 (1350.0ms) | 1 (0.0ms) | 87 (1220.0ms) | 87 (1200.0ms) |

## Python Ecosystem

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Pure Python sum(10k) | 70,764 (3.7ms) | 18,839 (15.2ms) | 74,956 (3.4ms) | 41,779 (27.5ms) |
| numpy mean(10k) | 8,399 (30.4ms) | 8,637 (29.9ms) | 34,775 (21.5ms) |
| numpy SVD 100x100 | 3,876 (65.8ms) | 4,071 (62.5ms) | 5,820 (66.5ms) |

## I/O Simulation

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| sleep(1ms) | 7,878 (32.4ms) | 51,620 (5.1ms) | 6,718 (37.9ms) | 92,257 (4.0ms) |

## JSON Parsing

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Parse 41B JSON | 211,792 (1.0ms) | 68,790 (4.6ms) | 210,906 (1.0ms) | 84,648 (41.0ms) |
| Parse 7KB JSON | 86,303 (2.9ms) | 19,956 (14.4ms) | 99,327 (2.5ms) | 56,688 (23.3ms) |
| Parse 93KB JSON | 10,119 (25.2ms) | 1,884 (161.9ms) | 10,461 (24.3ms) | 7,390 (55.8ms) |