#!/bin/bash

echo -n 111
sleep 0.1
echo -n aaa >&2
sleep 0.1
echo 333
sleep 0.1
echo bbb >&2
