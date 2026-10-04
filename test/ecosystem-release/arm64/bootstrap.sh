#!/bin/sh
ip link set lo up
echo DEVER_ARM_GUEST_BEGIN
sh /payload/run.sh
result=$?
echo DEVER_ARM_GUEST_EXIT=$result
sync
poweroff -f
