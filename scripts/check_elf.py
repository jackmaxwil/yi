#!/usr/bin/env python3
"""Static-ELF shape check for a cross-built binary the host cannot execute.

    scripts/check_elf.py <path> <target-triple>

Asserts ELF magic, 64-bit class, e_machine matches the target arch, and no
PT_INTERP program header (the "static, no dynamic linker" claim musl builds
make). Exit code is the verdict; this cannot run the binary, so shape is all
it proves.
"""
import struct
import sys

EM_X86_64 = 0x3E
EM_AARCH64 = 0xB7
ARCH_MACHINE = {
    "x86_64": EM_X86_64,
    "aarch64": EM_AARCH64,
}


def fail(msg: str) -> None:
    print(f"check_elf: {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    if len(sys.argv) != 3:
        fail("usage: check_elf.py <path> <target-triple>")
    path, target = sys.argv[1], sys.argv[2]
    arch = target.split("-", 1)[0]
    want_machine = ARCH_MACHINE.get(arch)
    if want_machine is None:
        fail(f"unknown arch {arch!r} in target {target!r} (known: {sorted(ARCH_MACHINE)})")

    data = open(path, "rb").read()
    if len(data) < 64 or data[0:4] != b"\x7fELF":
        fail(f"{path} is not an ELF file (bad magic)")
    ei_class = data[4]
    if ei_class != 2:
        fail(f"{path} is not 64-bit ELF (EI_CLASS={ei_class})")
    e_machine = struct.unpack_from("<H", data, 18)[0]
    if e_machine != want_machine:
        fail(f"{path} e_machine=0x{e_machine:x}, want 0x{want_machine:x} for {arch}")

    e_phoff, = struct.unpack_from("<Q", data, 32)
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 54)
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type, = struct.unpack_from("<I", data, off)
        if p_type == 3:  # PT_INTERP
            fail(f"{path} carries PT_INTERP: not statically linked")

    print(f"check_elf: {path} ok (ELF64, machine 0x{e_machine:x}, no PT_INTERP)")


if __name__ == "__main__":
    main()
