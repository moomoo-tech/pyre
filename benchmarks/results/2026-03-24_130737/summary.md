# Benchmark Summary

Generated: 2026-03-24T13:28:46.660630


## Basic Throughput

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Hello World | 211,019 (1.0ms) | 77,853 (4.3ms) | 216,674 (0.9ms) | 81,251 (24.5ms) |
| JSON small (3 fields) | 212,521 (0.9ms) | 73,777 (4.5ms) | 210,479 (0.9ms) | 80,672 (23.0ms) |
| JSON medium (100 users) | 63,649 (4.1ms) | 13,293 (21.9ms) | 67,817 (3.7ms) | 42,775 (23.2ms) |
| JSON large (500 records) | 5,063 (50.3ms) | 1,917 (160.5ms) | 4,835 (52.6ms) | 4,467 (89.2ms) |

## CPU Intensive

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| fib(10) | 205,092 (1.1ms) | 58,876 (5.5ms) | 205,694 (1.0ms) | 74,444 (16.3ms) |
| fib(20) | 10,830 (23.5ms) | 1,899 (164.6ms) | 11,065 (23.0ms) | 8,664 (60.8ms) |
| fib(30) | 90 (1250.0ms) | 1 (0.0ms) | 91 (1200.0ms) | 79 (935.2ms) |

## Python Ecosystem

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Pure Python sum(10k) | 74,894 (3.4ms) | 18,036 (16.0ms) | 74,362 (3.6ms) | 43,836 (19.1ms) |
| numpy mean(10k) | 8,290 (30.9ms) | 8,507 (30.0ms) | 31,261 (25.3ms) |
| numpy SVD 100x100 | 3,940 (64.6ms) | 3,993 (63.7ms) | 5,079 (62.5ms) |

## I/O Simulation

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| sleep(1ms) | 7,867 (32.5ms) | 46,704 (5.7ms) | 7,905 (32.1ms) | 77,967 (4.4ms) |

## JSON Parsing

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Parse 41B JSON | 203,262 (1.1ms) | 60,514 (5.3ms) | 208,249 (1.0ms) | 75,277 (20.2ms) |
| Parse 7KB JSON | 90,867 (2.9ms) | 19,873 (14.5ms) | 94,942 (2.9ms) | 50,520 (20.4ms) |
| Parse 93KB JSON | 9,750 (26.1ms) | 1,847 (163.4ms) | 9,958 (25.6ms) | 7,345 (58.8ms) |