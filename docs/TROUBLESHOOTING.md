# Troubleshooting

The log pane (GUI) and stdout (CLI) always contain the real reason. When in
doubt, press **Save log** and read it.

| Symptom | Likely cause | What to do |
|---|---|---|
| Stick does not appear in the list | Not elevated, or the disk is offline | Run as Administrator. Check Disk Management for an *Offline* disk. |
| "Repair refused (safety)" | The selected disk is not USB, or it hosts Windows | Select the correct USB disk. Never repair your system disk. |
| "The partition was created but Windows did not expose a volume" | Windows has not re-enumerated the disk | Unplug and replug the stick, then run Repair again. |
| "all formatting methods failed" | Stick is write-protected or physically failing | See the write-protection section below. |
| `format did not take effect: ... reports RAW` | The format command ran but produced no filesystem | Re-run Repair. If it persists, try the CLI with `--zero`. |
| FAT32 option rejected | Volume is larger than 32 GiB | Use exFAT. This is a Windows limit, not a bug. |
| "Access is denied" | Not elevated, or a volume is held open | Close Explorer windows for the stick and run as Administrator. |
| Disk shows as *write-protected* | Controller latch or registry policy | Run `diskpart`, `select disk N`, `attributes disk clear readonly`. If it returns, the controller has latched read-only and only a power cycle or the vendor's tool can clear it. |
| Disk is physically failing | I/O errors in the log | Replace the stick. No software can fix a failing controller. |

## Write-protected sticks

Some USB sticks and SD cards latch into read-only mode after a failed write.
Symptoms:

* `IOCTL_DISK_IS_WRITABLE` fails,
* `diskpart` says the disk is write-protected,
* the partition table can be read but never written.

Try, in order:

1. Run StickDoctor again (it clears the Windows read-only attribute).
2. Unplug, wait 10 seconds, plug into a different USB port.
3. Check the registry value
   `HKLM\SYSTEM\CurrentControlSet\Control\StorageDevicePolicies\WriteProtect`
   (should be `0` or absent).
4. For SD cards, check the physical lock switch.
5. If it still refuses, the controller has latched read-only. Only a power
   cycle (leave it unplugged for a while) or the vendor's low-level tool can
   reset it.

## Collecting a useful bug report

* Description of the stick (model, size, what image you wrote).
* The log from **Save log**.
* Windows version (`winver`).
* Whether the stick appears in Disk Management and what it says there.