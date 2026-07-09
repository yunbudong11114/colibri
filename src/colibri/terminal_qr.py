from __future__ import annotations

from dataclasses import dataclass


_VERSION = 5
_SIZE = 17 + 4 * _VERSION
_DATA_CODEWORDS = 108
_ECC_CODEWORDS = 26
_FORMAT_L_MASK_0 = 0b111011111000100


@dataclass
class _Matrix:
    modules: list[list[bool | None]]
    reserved: list[list[bool]]

    @classmethod
    def empty(cls) -> _Matrix:
        return cls(
            modules=[[None for _ in range(_SIZE)] for _ in range(_SIZE)],
            reserved=[[False for _ in range(_SIZE)] for _ in range(_SIZE)],
        )

    def set(self, x: int, y: int, value: bool, *, reserve: bool = True) -> None:
        if 0 <= x < _SIZE and 0 <= y < _SIZE:
            self.modules[y][x] = value
            if reserve:
                self.reserved[y][x] = True


def render_terminal_qr(text: str, *, quiet_zone: int = 2) -> str | None:
    matrix = _encode_version5_l(text)
    if matrix is None:
        return None
    lines: list[str] = []
    white = "  "
    black = "██"
    width = _SIZE + quiet_zone * 2
    lines.extend([white * width for _ in range(quiet_zone)])
    for row in matrix.modules:
        line = white * quiet_zone
        line += "".join(black if value else white for value in row)
        line += white * quiet_zone
        lines.append(line)
    lines.extend([white * width for _ in range(quiet_zone)])
    return "\n".join(lines)


def _encode_version5_l(text: str) -> _Matrix | None:
    data = text.encode("utf-8")
    if len(data) > 106:
        return None
    codewords = _make_data_codewords(data)
    codewords.extend(_reed_solomon_remainder(codewords, _ECC_CODEWORDS))
    bits = [(codeword >> bit) & 1 == 1 for codeword in codewords for bit in range(7, -1, -1)]

    matrix = _Matrix.empty()
    _draw_function_patterns(matrix)
    _draw_codewords(matrix, bits)
    _apply_mask_0(matrix)
    _draw_format_bits(matrix)
    return matrix


def _make_data_codewords(data: bytes) -> list[int]:
    bits: list[bool] = []
    _append_bits(bits, 0b0100, 4)
    _append_bits(bits, len(data), 8)
    for byte in data:
        _append_bits(bits, byte, 8)
    remaining = _DATA_CODEWORDS * 8 - len(bits)
    _append_bits(bits, 0, min(4, remaining))
    while len(bits) % 8:
        bits.append(False)
    codewords = [_bits_to_int(bits[index : index + 8]) for index in range(0, len(bits), 8)]
    pad = 0xEC
    while len(codewords) < _DATA_CODEWORDS:
        codewords.append(pad)
        pad = 0x11 if pad == 0xEC else 0xEC
    return codewords


def _append_bits(bits: list[bool], value: int, width: int) -> None:
    for bit in range(width - 1, -1, -1):
        bits.append(((value >> bit) & 1) == 1)


def _bits_to_int(bits: list[bool]) -> int:
    value = 0
    for bit in bits:
        value = (value << 1) | int(bit)
    return value


def _draw_function_patterns(matrix: _Matrix) -> None:
    _draw_finder(matrix, 0, 0)
    _draw_finder(matrix, _SIZE - 7, 0)
    _draw_finder(matrix, 0, _SIZE - 7)
    for index in range(8, _SIZE - 8):
        value = index % 2 == 0
        matrix.set(index, 6, value)
        matrix.set(6, index, value)
    _draw_alignment(matrix, 30, 30)
    matrix.set(8, 4 * _VERSION + 9, True)
    _reserve_format_areas(matrix)


def _draw_finder(matrix: _Matrix, x: int, y: int) -> None:
    for dy in range(-1, 8):
        for dx in range(-1, 8):
            xx = x + dx
            yy = y + dy
            if not (0 <= xx < _SIZE and 0 <= yy < _SIZE):
                continue
            value = 0 <= dx <= 6 and 0 <= dy <= 6 and (dx in {0, 6} or dy in {0, 6} or 2 <= dx <= 4 and 2 <= dy <= 4)
            matrix.set(xx, yy, value)


def _draw_alignment(matrix: _Matrix, center_x: int, center_y: int) -> None:
    for dy in range(-2, 3):
        for dx in range(-2, 3):
            distance = max(abs(dx), abs(dy))
            matrix.set(center_x + dx, center_y + dy, distance != 1)


def _reserve_format_areas(matrix: _Matrix) -> None:
    for index in range(9):
        if index != 6:
            matrix.set(8, index, False)
            matrix.set(index, 8, False)
    for index in range(8):
        matrix.set(_SIZE - 1 - index, 8, False)
        matrix.set(8, _SIZE - 1 - index, False)


def _draw_codewords(matrix: _Matrix, bits: list[bool]) -> None:
    bit_index = 0
    direction = -1
    x = _SIZE - 1
    y = _SIZE - 1
    while x > 0:
        if x == 6:
            x -= 1
        while 0 <= y < _SIZE:
            for dx in range(2):
                xx = x - dx
                if not matrix.reserved[y][xx]:
                    matrix.modules[y][xx] = bits[bit_index] if bit_index < len(bits) else False
                    bit_index += 1
            y += direction
        direction = -direction
        y += direction
        x -= 2


def _apply_mask_0(matrix: _Matrix) -> None:
    for y in range(_SIZE):
        for x in range(_SIZE):
            if not matrix.reserved[y][x] and (x + y) % 2 == 0:
                matrix.modules[y][x] = not matrix.modules[y][x]


def _draw_format_bits(matrix: _Matrix) -> None:
    for index in range(15):
        bit = ((_FORMAT_L_MASK_0 >> index) & 1) == 1
        if index < 6:
            matrix.set(8, index, bit)
        elif index < 8:
            matrix.set(8, index + 1, bit)
        else:
            matrix.set(8, _SIZE - 15 + index, bit)

        if index < 8:
            matrix.set(_SIZE - 1 - index, 8, bit)
        elif index == 8:
            matrix.set(7, 8, bit)
        else:
            matrix.set(14 - index, 8, bit)


def _reed_solomon_remainder(data: list[int], degree: int) -> list[int]:
    generator = _reed_solomon_generator(degree)
    result = [0] * degree
    for byte in data:
        factor = byte ^ result.pop(0)
        result.append(0)
        for index, coefficient in enumerate(generator):
            result[index] ^= _gf_multiply(coefficient, factor)
    return result


def _reed_solomon_generator(degree: int) -> list[int]:
    result = [1]
    for exponent in range(degree):
        result = _poly_multiply(result, [1, _gf_pow(2, exponent)])
    return result[1:]


def _poly_multiply(left: list[int], right: list[int]) -> list[int]:
    result = [0] * (len(left) + len(right) - 1)
    for left_index, left_value in enumerate(left):
        for right_index, right_value in enumerate(right):
            result[left_index + right_index] ^= _gf_multiply(left_value, right_value)
    return result


def _gf_pow(value: int, power: int) -> int:
    result = 1
    for _ in range(power):
        result = _gf_multiply(result, value)
    return result


def _gf_multiply(left: int, right: int) -> int:
    result = 0
    while right:
        if right & 1:
            result ^= left
        left <<= 1
        if left & 0x100:
            left ^= 0x11D
        right >>= 1
    return result & 0xFF
