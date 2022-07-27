#!/bin/sh
# It's often useful to have something that outputs to both stdout and stderr

echo 1 stdout
echo 2 STDERR >&2
echo 3 stdout

echo 4 STDERR: sleep 1.. >&2
sleep 1

echo 5 stdout: sleep 1..
sleep 1

echo 6 STDERR >&2
echo 7 stdout
echo 8 STDERR >&2
