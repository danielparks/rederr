#!/bin/sh
# It's often useful to have something that outputs to both stdout and stderr

echo 1 stdout
echo 2 STDERR >&2
echo 3 stdout
echo 4 STDERR >&2
echo 5 stdout: sleep 1..

sleep 1

echo 6 stdout
echo 7 STDERR >&2
echo 8 stdout
echo 9 STDERR >&2
