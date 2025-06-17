#!/bin/sh

if [ $# -lt 2 ]; then
  echo "Usage: $0 <Prefix> <N> [...]" >&2
  exit 1
fi

prefix=$1
total=$2
shift
shift

#trap "kill $(jobs -p); exit 1" INT

c=0
list=""
num=1
while [ $num -le $total ]; do
  list="$list -u $prefix$num"
  c=$((c + 1))
  if [ $c -ge 50 ]; then
    ($@ $list) &
    list=""
    c=0
  fi
  num=$((num + 1))
done

($@ $list) &

wait
