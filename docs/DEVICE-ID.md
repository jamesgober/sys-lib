# Device identity

An identifier for "this device" that an application can store and recognise later: stable across reboots, network changes, and application reinstalls; useless for correlating one machine across unrelated applications; impossible to turn back into the serial numbers behind it.

The feature is `device-id`, off by default. The API is in [`API.md`](./API.md#device-device-identity).

## When not to use it

- **Not authentication.** A device id is a claim, not proof. Root or Administrator can set `/etc/machine-id` or `MachineGuid` to anything, and hypervisors set firmware UUIDs freely. For proof of a device, use a key held in a TPM or Secure Enclave.
- **Not for tracking across applications or for advertising.** The application-specific key prevents correlation by design. Using device identifiers that way breaks platform policies and, in the EU, likely needs consent under the ePrivacy Directive, Article 5(3).
- **Personal data.** Under the GDPR (Recital 30), an identifier from a user's device can identify a person: an application storing a device id needs a lawful basis, a retention period, and a way to delete it. This is a statement of the issue, not legal advice.
- **Not stable enough for licensing alone.** Reinstalls and board replacements happen. Licensing should allow re-activation.

## How an id is made

```
 app_key  = secret                                        if the caller supplied a 32-byte secret
          = BLAKE3("sys-lib/device/v1/app-key" || 0x00 || u16_be(len(app)) || app)   otherwise
 message  = "sys-lib/device/v1" || 0x00 || basis tag || anchor tag || u16_be(len(canonical)) || canonical
 tag      = BLAKE3-keyed(app_key, message)
 DeviceId = first 16 bytes of tag, with RFC 9562 version 8 and variant bits set   (122 bits)
```

Basis tags: `Install` 1, `Hardware` 2; a fingerprint entry uses 0, so ids and entries never collide. Anchor tags are 1 to 6, in the order below.

**One anchor per id.** Concatenating anchors would make an id less stable: any one change, a network card say, would change the whole id. `device::id` hashes one anchor; a [fingerprint](#fingerprints) hashes each one separately.

**The derivation is frozen** for all of 1.x. A given machine and application id give the same device id in every 1.x release; a test pins an exact value. A change could only arrive as a new `Basis` or a new version tag.

**Without a secret**, anyone who knows the raw machine id and your (public) application id can compute the device id: the same property as systemd's application-specific machine id. **With a secret**, nobody without it can compute or correlate the id, even with root on the machine. Use one when ids are sent to a server.

**Raw values** go into a fixed buffer, are hashed, and the buffer is wiped. They never cross the public API: not in a return value, an error, `Debug`, `Display`, or a log.

## Anchors

| Anchor | Linux | macOS | Windows | Survives reinstall | Privilege |
|---|---|---|---|---|---|
| `InstallId` | `/etc/machine-id`, then `/var/lib/dbus/machine-id` | none | `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` (64-bit view) | no | none |
| `PlatformUuid` | `/sys/class/dmi/id/product_uuid` (0400) | `IOPlatformUUID` | SMBIOS type 1 UUID | yes | Linux: root |
| `BoardSerial` | `/sys/class/dmi/id/board_serial` (0400) | `IOPlatformSerialNumber` | SMBIOS type 2 serial | yes | Linux: root |
| `ProductSerial` | `/sys/class/dmi/id/product_serial` (0400) | not used twice | SMBIOS type 1 serial | yes | Linux: root |
| `CpuSignature` | vendor, CPUID signature, brand | same | same | yes | none; **not unique** |
| `PermanentMac` | lowest factory MAC of a physical interface | same | same | yes | none |

`Basis::Install` uses `InstallId`; macOS has none, so it uses the platform UUID. `Basis::Hardware` uses `PlatformUuid`, then `BoardSerial`.

**Canonical forms**, so one machine gives one id however a platform spells it: UUIDs as 16 bytes in RFC 9562 order (a raw SMBIOS table stores the first three fields little-endian and is converted, so `Basis::Hardware` gives the same id under Linux as root and under Windows on a dual-boot machine); machine ids as 16 bytes parsed from hex; serials trimmed and upper-cased; a MAC as 6 bytes.

**Placeholders** count as absent: all-zero and all-`F` UUIDs; serials such as `To be filled by O.E.M.`, `Default string`, `System Serial Number`, `Not Specified`, `None`, `0`, `123456789`; machine ids that are empty, `uninitialized`, or all zeros; MACs that are all zeros, broadcast, multicast, or locally administered. WSL 2's `eth0` MAC changes on every boot, so `PermanentMac` is not used there.

## Stability

`device::report()` says how far to trust the id on this machine without revealing any value: the state of each source (`Present`, `Absent`, `Placeholder`, `PermissionDenied`, `Unsupported`), `clone_risk`, `confidence`, and `changes_on`.

| Clone risk | Means | Fix |
|---|---|---|
| `Container` | every container of one image may share an id | do not rely on the id, or give each container its own machine id |
| `SharedImageLikely` | a container with a machine id, which came from its image | as above |
| `VirtualMachine` | clones of a template share an id until it is regenerated | `systemd-machine-id-setup` on Linux, `sysprep /generalize` on Windows |
| `MachineIdUninitialized` | `/etc/machine-id` is empty or `uninitialized` | `systemd-machine-id-setup` |

The id on the default basis changes when the OS is reinstalled or an administrator resets it (on macOS, when the logic board is replaced), and with the VM or container when those are created again.

Common failures:

- WSL without systemd may have no `/etc/machine-id`: `device::id` returns `NoIdentifierSource`, and the report says why.
- WSL 2 has no DMI: `Basis::Hardware` is unavailable, and the report says `Unsupported`.
- Linux without root: `Basis::Hardware` returns `NoIdentifierSource`, and the report says `PermissionDenied` for the DMI anchors. `Builder::fallback(true)` falls back to the install id, which is a different id; that is why it is off by default.

## Fingerprints

A fingerprint hashes every available anchor separately, so a stored one can tell "this machine, one part changed" (a network card swap, a reinstall) from "another machine".

| Anchor | Weight |
|---|---|
| `InstallId`, `PlatformUuid` | 3 |
| `BoardSerial`, `ProductSerial` | 2 |
| `PermanentMac`, `CpuSignature` | 1 |

`Same`: every anchor present in both matches, and more than the CPU signature was compared (identical CPUs share it, so alone it proves nothing). `LikelySame`: matched weight of at least 5, with at most one strong anchor (weight 2 or more) changed. Otherwise `Different`. Each hash is compared in constant time.

Stored format, 112 bytes:

```
 offset  size  field
 0       4     magic "SLFP"
 4       1     format version (1)
 5       1     anchor-present bitmap (bit n = Anchor n)
 6       2     reserved, zero
 8       96    6 x 16-byte anchor hashes (zeros where absent)
 104     8     first 8 bytes of BLAKE3(bytes 0..104)
```

`Fingerprint::from_bytes` checks length, magic, version, reserved bytes, bitmap consistency, and checksum, in that order, and returns `Malformed` for anything wrong. The checksum catches storage corruption, not tampering.
