"""Regenerate tiny, anonymous golden files from documented EDF/Level-5 layouts.

Development-only utility using Python's standard library. Rust tests consume
the checked-in files directly and the application has no Python dependency.
"""
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parent


def field(value, width):
    return str(value).encode("ascii").ljust(width, b" ")


def edf():
    fixed = b"".join(field(value, width) for value, width in [
        (0, 8), ("synthetic-anonymous", 80), ("synthetic-recording", 80),
        ("01.01.26", 8), ("00.00.00", 8), (768, 8), ("", 44),
        (2, 8), (1, 8), (2, 4),
    ])
    signals = b"".join(field(value, width) for values, width in [
        (["Fp1", "Fp2"], 16), (["", ""], 80), (["uV", "uV"], 8),
        ([-32768, -32768], 8), ([32767, 32767], 8),
        ([-32768, -32768], 8), ([32767, 32767], 8),
        (["HP:0.1Hz", "HP:0.1Hz"], 80), ([2, 2], 8), (["", ""], 32),
    ] for value in values)
    # Each record carries two samples of Fp1, then two samples of Fp2.
    return fixed + signals + struct.pack("<8h", 1, 2, -1, -2, 3, 4, -3, -4)


def element(kind, data):
    return struct.pack("<II", kind, len(data)) + data + bytes((-len(data)) % 8)


def matrix(name, rows, columns, values):
    return element(14, element(6, struct.pack("<II", 6, 0))
                   + element(5, struct.pack("<ii", rows, columns))
                   + element(1, name.encode("ascii"))
                   + element(9, struct.pack("<" + "d" * len(values), *values)))


def mat(compressed):
    header = b"MATLAB 5.0 MAT-file, synthetic P3 golden data".ljust(116, b" ")
    header += bytes(8) + b"\x00\x01IM"
    arrays = [matrix("eeg", 2, 4, [1, -1, 2, -2, 3, -3, 4, -4]),
              matrix("fs", 1, 1, [2])]
    if compressed:
        # miCOMPRESSED is the exception to the 8-byte padding rule.
        arrays = [struct.pack("<II", 15, len(data)) + data
                  for data in map(zlib.compress, arrays)]
    return header + b"".join(arrays)


if __name__ == "__main__":
    (ROOT / "golden.edf").write_bytes(edf())
    (ROOT / "golden.mat").write_bytes(mat(False))
    (ROOT / "golden-compressed.mat").write_bytes(mat(True))
    (ROOT / "golden.csv").write_text("Fp1,Fp2\n1,-1\n2,-2\n3,-3\n4,-4\n")
