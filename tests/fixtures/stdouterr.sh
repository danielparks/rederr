#!/bin/bash
# Used for testing by hand, not automated testing.

echo 01 stdout
echo 02 stdout
echo 03 STDERR >&2

echo 04 STDERR sleep 0.1 >&2
sleep 0.1

echo 05 stdout sleep 0.1
sleep 0.1

echo -n "06 STDERR " >&2
echo -n "07 stdout "
echo 08 STDERR >&2

echo -n "09 STDERR sleep 0.2 " >&2
sleep 0.2
echo -n "10 stdout sleep 0.1 "
sleep 0.1
echo 11 STDERR >&2

kill $$
