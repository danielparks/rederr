#!/bin/zsh
# It's often useful to have something that outputs to both stdout and stderr

echo 1 stdout
echo 2 stdout
echo 3 STDERR >&2

echo 4 STDERR: sleep 0.1 >&2
sleep 0.1

echo 5 stdout: sleep 0.1
sleep 0.1

echo -n "6 STDERR " >&2
echo -n "7 stdout "
echo 8 STDERR >&2

kill $$
