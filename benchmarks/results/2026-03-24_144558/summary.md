# Benchmark Summary

Generated: 2026-03-24T15:07:07.191314


## Basic Throughput

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Hello World | 213,887 (0.9ms) | 83,097 (3.9ms) | 215,090 (0.9ms) | 85,935 (20.1ms) |
| JSON small (3 fields) | 209,414 (1.0ms) | 65,302 (5.1ms) | 214,984 (0.9ms) | 86,468 (26.3ms) |
| JSON medium (100 users) | 62,972 (4.2ms) | 12,202 (25.7ms) | 61,742 (4.2ms) | 44,882 (25.8ms) |
| JSON large (500 records) | 5,183 (49.1ms) | 1,930 (161.3ms) | 4,930 (51.4ms) | 4,948 (81.3ms) |

## CPU Intensive

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| fib(10) | 206,179 (1.0ms) | 57,243 (5.7ms) | 197,713 (1.1ms) | 86,786 (37.5ms) |
| fib(20) | 10,396 (25.2ms) | 1,859 (171.4ms) | 11,145 (22.8ms) | 10,933 (36.5ms) |
| fib(30) | 85 (1240.0ms) | 1 (0.0ms) | 85 (1260.0ms) | 88 (560.6ms) |

## Python Ecosystem

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Pure Python sum(10k) | 73,805 (3.6ms) | 17,716 (16.6ms) | 74,512 (3.4ms) | 46,959 (22.8ms) |
| numpy mean(10k) | 8,209 (31.1ms) | 8,791 (29.1ms) | 35,561 (24.4ms) |
| numpy SVD 100x100 | 4,043 (63.0ms) | 3,916 (64.8ms) | 5,812 (49.2ms) |

## I/O Simulation

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| sleep(1ms) | 7,793 (32.7ms) | 52,494 (5.0ms) | 6,734 (37.8ms) | 85,350 (4.1ms) |

## JSON Parsing

| Scenario | pyre_subinterp | pyre_gil | pyre_hybrid | robyn |
|---|---|---|---|---|
| Parse 41B JSON | 205,675 (1.1ms) | 72,788 (4.4ms) | 210,170 (1.0ms) | 83,708 (34.6ms) |
| Parse 7KB JSON | 89,975 (2.9ms) | 20,231 (14.3ms) | 96,142 (2.6ms) | 56,193 (23.8ms) |
| Parse 93KB JSON | 9,877 (25.8ms) | 1,901 (163.9ms) | 9,934 (25.6ms) | 8,893 (42.7ms) |