#!/bin/busybox sh
set -eu
/bin/busybox --install -s /bin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /tmp /payload /run
mount -t devpts devpts /dev/pts
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /dev/shm
chmod 1777 /tmp /dev/shm
modprobe virtio_mmio
modprobe virtio_blk
modprobe ext4
mount -o ro /dev/vda /payload
# initramfs 的 rootfs 不支持 pivot_root；先进入正常根挂载再验收嵌套沙箱。
mkdir -p /newroot
mount -t tmpfs tmpfs /newroot
cp -a /bin /sbin /lib /etc /newroot/
cp /init-stage /newroot/init-stage
mkdir -p /newroot/proc /newroot/sys /newroot/dev /newroot/tmp /newroot/payload /newroot/run
mount -o move /proc /newroot/proc
mount -o move /sys /newroot/sys
mount -o move /dev /newroot/dev
mount -o move /tmp /newroot/tmp
mount -o move /payload /newroot/payload
exec /bin/busybox switch_root /newroot /bin/sh /init-stage
