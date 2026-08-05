#!/usr/bin/env bash
#
# Encrypt a removable USB stick with LUKS2, prompting for the passphrase.
#
#   sudo ./encrypt-usb.sh /dev/sdX
#
# THIS DESTROYS EVERYTHING ON THE TARGET. There is no undo, and no recovery
# without the passphrase — LUKS has no back door and neither does this script.
#
# What it does: wipes the signatures, writes one partition, puts a LUKS2
# container in it, formats ext4 inside that, and leaves it mounted so you can
# check it. What it deliberately does NOT do: pick the device for you.

set -euo pipefail

die() { printf '\n%s\n' "$*" >&2; exit 1; }

# ── the target has to be named, and has to survive four checks ──────────────

[[ $# -eq 1 ]] || die "usage: sudo $0 /dev/sdX     (run lsblk first and be sure)"
device="$1"
name="usbcrypt"                       # the mapper name while it is open
label="${LABEL:-SECURE}"              # filesystem label; override with LABEL=

[[ $EUID -eq 0 ]] || die "run this with sudo — it partitions a disk."
[[ -b $device ]] || die "$device is not a block device."

base="$(basename "$device")"

# A partition, not a disk: refusing rather than helpfully targeting the parent,
# because "helpfully" is how the wrong thing gets wiped.
[[ -e "/sys/block/$base" ]] \
  || die "$device looks like a partition. Give the whole disk, e.g. /dev/sdb."

# Removable only. This is the check that stands between a USB stick and a
# system disk, so it is not optional and there is no flag to skip it.
[[ "$(cat "/sys/block/$base/removable" 2>/dev/null)" == "1" ]] \
  || die "$device is not removable. Refusing — this script is for USB media."

# Not the disk we booted from, however removable it claims to be.
root_src="$(findmnt -no SOURCE / || true)"
if [[ -n $root_src ]]; then
  root_disk="$(lsblk -no PKNAME "$root_src" 2>/dev/null | head -1 || true)"
  [[ "$root_disk" != "$base" ]] || die "$device holds the root filesystem. Absolutely not."
fi

# ── show what is about to be lost ───────────────────────────────────────────

size="$(lsblk -dno SIZE "$device")"
model="$(cat "/sys/block/$base/device/model" 2>/dev/null | xargs || echo unknown)"

printf '\nAbout to ERASE and encrypt:\n\n'
printf '  device   %s\n' "$device"
printf '  size     %s\n' "$size"
printf '  model    %s\n\n' "$model"
lsblk -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$device"
printf '\nEverything above is destroyed. This cannot be undone.\n'

read -r -p "Type the device path again to confirm: " confirm
[[ "$confirm" == "$device" ]] || die "Did not match. Nothing was written."

# ── unmount anything already mounted from it ────────────────────────────────

while read -r part; do
  [[ -n $part ]] || continue
  if findmnt -no TARGET "$part" >/dev/null 2>&1; then
    echo "unmounting $part"
    umount "$part"
  fi
done < <(lsblk -lno PATH "$device" | tail -n +2)

# ── wipe, partition, encrypt ────────────────────────────────────────────────

echo "wiping existing signatures…"
wipefs --all --force "$device" >/dev/null

echo "writing a single partition…"
# GPT, one partition filling the disk. sfdisk reads the layout from stdin, so
# the whole table is one line and there is nothing interactive to get wrong.
echo 'label: gpt' | sfdisk --quiet "$device"
echo ',,L' | sfdisk --quiet --append "$device"
partprobe "$device"
sleep 1

# /dev/sdb1, or /dev/nvme0n1p1 — the p is only there for some device classes.
part="$(lsblk -lno PATH "$device" | tail -n +2 | head -1)"
[[ -b $part ]] || die "no partition appeared on $device"

echo
echo "Now choose the passphrase for $part."
echo "Nothing recovers this volume without it — not the maker, not this script."
echo

# cryptsetup does its own prompting: asks twice, checks they match, never
# echoes, and never puts the passphrase in the process table or your history.
cryptsetup luksFormat --type luks2 --verify-passphrase "$part"

echo
echo "Unlocking to build the filesystem…"
cryptsetup open "$part" "$name"

# The mapper closes on any failure past this point, so a half-built volume is
# not left unlocked and attached.
trap 'cryptsetup close "$name" 2>/dev/null || true' ERR

echo "formatting ext4…"
mkfs.ext4 -q -L "$label" "/dev/mapper/$name"

# Owned by whoever invoked sudo, so it is writable without root afterwards.
mountpoint="/run/media/${SUDO_USER:-root}/$label"
mkdir -p "$mountpoint"
mount "/dev/mapper/$name" "$mountpoint"
chown "${SUDO_UID:-0}:${SUDO_GID:-0}" "$mountpoint"

trap - ERR

printf '\nDone.\n\n'
printf '  mounted at   %s\n' "$mountpoint"
printf '  unlocked as  /dev/mapper/%s\n\n' "$name"
printf 'When finished:\n'
printf '  sudo umount %s\n' "$mountpoint"
printf '  sudo cryptsetup close %s\n\n' "$name"
printf 'To use it again later:\n'
printf '  sudo cryptsetup open %s %s\n' "$part" "$name"
printf '  sudo mount /dev/mapper/%s /mnt\n\n' "$name"
printf 'Most desktops detect a LUKS stick on insert and just ask for the\n'
printf 'passphrase, so the manual commands are a fallback.\n'
