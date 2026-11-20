# Statime on macOS (experimental)

This fork can run as a PTPv1 follower on macOS for Dante networks. It feeds the
clock to [Inferno](https://github.com/Naxii-e/inferno) (branch `macos`) through
usrvclock. It is meant as the clock daemon for a macOS Dante virtual soundcard.

macOS support is limited on purpose:

* The kernel clock is never modified. Statime keeps a virtual clock based on
  `CLOCK_UPTIME_RAW` and only exports it over usrvclock (`/tmp/ptp-usrvclock`).
  The config must set `virtual-system-clock = true`,
  `virtual-system-clock-base = "uptime_raw"` and `usrvclock-export = true`.
  Any other combination is rejected at startup.
* Only `network-mode = "ipv4"` / `"ipv6"` and `hardware-clock = "none"` are
  supported. There is no Ethernet (layer 2) mode and no PHC.
* Receive timestamps come from `SO_TIMESTAMP`, a software timestamp taken by
  the kernel. macOS has no send timestamps, so the clock is read right after
  `send()` returns.

## Build

```
git submodule update --init
cargo build --bin statime
```

The binary ends up at `target/debug/statime`. If linking fails with the
Command Line Tools SDK, point the build at the Xcode SDK, e.g.
`SDKROOT=$(xcrun --sdk macosx --show-sdk-path) cargo build --bin statime`
with Xcode selected via `xcode-select`.

## Run

1. Set `interface` in `inferno-ptpv1-macos.toml` to the interface on the
   Dante network (`networksetup -listallhardwareports`).
2. Stop Audinate's services on this Mac. They hold the same UDP ports
   (PTP 319/320, and 8700/8800 which Inferno needs):
   ```
   sudo launchctl bootout system/com.audinate.dante.DanteVirtualSoundcard
   sudo launchctl bootout system/com.audinate.dante.ConMon
   ```
   While ConMon is stopped, Dante Controller on this Mac shows no devices.
   Use Dante Controller on another machine, or Inferno's
   `rx_subscriptions.toml`. To restore the services:
   ```
   sudo launchctl bootstrap system /Library/LaunchDaemons/com.audinate.dante.DanteVirtualSoundcard.plist
   sudo launchctl bootstrap system /Library/LaunchDaemons/com.audinate.dante.ConMon.plist
   ```
3. Start statime (root is needed for ports 319/320):
   ```
   sudo target/debug/statime -c inferno-ptpv1-macos.toml
   ```
4. Start Inferno, e.g. `INFERNO_BIND_IP=en8 ./target/debug/inferno2pipe -c 2 -o rec.raw`.
   It logs `clock ready` once the usrvclock socket delivers.

## Measurements (2026-11-20)

Setup:

* Mac running macOS 27.0, wired to the Dante network on `en8`.
* PTPv1 leader: a Dante hardware device (sync interval 250 ms).
* Transmitter: a Linux Inferno host sending 2ch over unicast.
* Recording: `inferno2pipe -c 2` for 3600 s (48 kHz, 32-bit).

| Item | Result | Target |
|---|---|---|
| File size | 1,381,577,424 bytes (−0.06 % of 1,382,400,000) | ±0.1 % |
| Loss logs | 1 event (2,490 samples per channel, reorder buffer timeout, 2 s after start) | 0, a few at the first connection allowed |
| Offset mean | −25 µs | — |
| Offset std dev | 124 µs | < 1 ms |
| Offset \|p99\| / max | 477 µs / 1.21 ms | — |
| GUARD warnings during the recording | 2 episodes (18 log lines) | recorded only |

The shortfall in file size is the time between start and the first subscribed
packet (about 2 s). The offset statistics cover 17,639 sync measurements taken
during the recording. Each GUARD episode followed a single outlier measurement
of about +0.9 to +1.1 ms that made the clock jump. Inferno logged no holes or
loss at those times.

A 30 s run beforehand also showed audio on both channels and no loss.

## Open issues

* Software timestamps give an offset spread of about 0.1 ms, with rare
  outliers around 1 ms. That is enough for receiving with Inferno, but much
  worse than PTP on a Linux NIC with hardware timestamps. Filtering outliers
  before they reach the clock would remove the GUARD episodes.
* Statime, DVS and ConMon cannot run on the same Mac at the same time, so
  Dante Controller on the same Mac does not work while this runs.
* Only PTPv1 follower mode has been tested. Acting as leader and PTPv2 are
  untested on macOS.
