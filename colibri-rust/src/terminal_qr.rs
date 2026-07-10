const VERSION: usize = 5;
const SIZE: usize = 17 + 4 * VERSION;
const DATA_CODEWORDS: usize = 108;
const ECC_CODEWORDS: usize = 26;
const FORMAT_L_MASK_0: u16 = 0b111011111000100;

#[derive(Clone)]
struct Matrix {
    modules: Vec<Vec<Option<bool>>>,
    reserved: Vec<Vec<bool>>,
}

impl Matrix {
    fn empty() -> Self {
        Self {
            modules: vec![vec![None; SIZE]; SIZE],
            reserved: vec![vec![false; SIZE]; SIZE],
        }
    }

    fn set(&mut self, x: isize, y: isize, value: bool, reserve: bool) {
        if x < 0 || y < 0 || x >= SIZE as isize || y >= SIZE as isize {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        self.modules[y][x] = Some(value);
        if reserve {
            self.reserved[y][x] = true;
        }
    }
}

pub fn render_terminal_qr(text: &str) -> Option<String> {
    render_terminal_qr_with_quiet_zone(text, 2)
}

pub fn render_terminal_qr_with_quiet_zone(text: &str, quiet_zone: usize) -> Option<String> {
    let matrix = encode_version5_l(text)?;
    let white = "  ";
    let black = "██";
    let width = SIZE + quiet_zone * 2;
    let mut lines = Vec::new();
    for _ in 0..quiet_zone {
        lines.push(white.repeat(width));
    }
    for row in matrix.modules {
        let mut line = white.repeat(quiet_zone);
        for value in row {
            line.push_str(if value.unwrap_or(false) { black } else { white });
        }
        line.push_str(&white.repeat(quiet_zone));
        lines.push(line);
    }
    for _ in 0..quiet_zone {
        lines.push(white.repeat(width));
    }
    Some(lines.join("\n"))
}

fn encode_version5_l(text: &str) -> Option<Matrix> {
    let data = text.as_bytes();
    if data.len() > 106 {
        return None;
    }
    let mut codewords = make_data_codewords(data);
    let ecc = reed_solomon_remainder(&codewords, ECC_CODEWORDS);
    codewords.extend(ecc);
    let bits = codewords
        .iter()
        .flat_map(|codeword| (0..8).rev().map(move |bit| ((codeword >> bit) & 1) == 1))
        .collect::<Vec<_>>();

    let mut matrix = Matrix::empty();
    draw_function_patterns(&mut matrix);
    draw_codewords(&mut matrix, &bits);
    apply_mask_0(&mut matrix);
    draw_format_bits(&mut matrix);
    Some(matrix)
}

fn make_data_codewords(data: &[u8]) -> Vec<u8> {
    let mut bits = Vec::new();
    append_bits(&mut bits, 0b0100, 4);
    append_bits(&mut bits, data.len() as u32, 8);
    for byte in data {
        append_bits(&mut bits, *byte as u32, 8);
    }
    let remaining = DATA_CODEWORDS * 8 - bits.len();
    append_bits(&mut bits, 0, remaining.min(4));
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut codewords = bits
        .chunks(8)
        .map(bits_to_int)
        .map(|value| value as u8)
        .collect::<Vec<_>>();
    let mut pad = 0xEC;
    while codewords.len() < DATA_CODEWORDS {
        codewords.push(pad);
        pad = if pad == 0xEC { 0x11 } else { 0xEC };
    }
    codewords
}

fn append_bits(bits: &mut Vec<bool>, value: u32, width: usize) {
    for bit in (0..width).rev() {
        bits.push(((value >> bit) & 1) == 1);
    }
}

fn bits_to_int(bits: &[bool]) -> u32 {
    bits.iter()
        .fold(0, |value, bit| (value << 1) | u32::from(*bit))
}

fn draw_function_patterns(matrix: &mut Matrix) {
    draw_finder(matrix, 0, 0);
    draw_finder(matrix, (SIZE - 7) as isize, 0);
    draw_finder(matrix, 0, (SIZE - 7) as isize);
    for index in 8..SIZE - 8 {
        let value = index % 2 == 0;
        matrix.set(index as isize, 6, value, true);
        matrix.set(6, index as isize, value, true);
    }
    draw_alignment(matrix, 30, 30);
    matrix.set(8, (4 * VERSION + 9) as isize, true, true);
    reserve_format_areas(matrix);
}

fn draw_finder(matrix: &mut Matrix, x: isize, y: isize) {
    for dy in -1..8 {
        for dx in -1..8 {
            let value = (0..=6).contains(&dx)
                && (0..=6).contains(&dy)
                && (dx == 0
                    || dx == 6
                    || dy == 0
                    || dy == 6
                    || ((2..=4).contains(&dx) && (2..=4).contains(&dy)));
            matrix.set(x + dx, y + dy, value, true);
        }
    }
}

fn draw_alignment(matrix: &mut Matrix, center_x: isize, center_y: isize) {
    for dy in -2isize..=2 {
        for dx in -2isize..=2 {
            let distance = dx.abs().max(dy.abs());
            matrix.set(center_x + dx, center_y + dy, distance != 1, true);
        }
    }
}

fn reserve_format_areas(matrix: &mut Matrix) {
    for index in 0..9 {
        if index != 6 {
            matrix.set(8, index, false, true);
            matrix.set(index, 8, false, true);
        }
    }
    for index in 0..8 {
        matrix.set((SIZE - 1) as isize - index, 8, false, true);
        matrix.set(8, (SIZE - 1) as isize - index, false, true);
    }
}

fn draw_codewords(matrix: &mut Matrix, bits: &[bool]) {
    let mut bit_index = 0usize;
    let mut direction = -1isize;
    let mut x = (SIZE - 1) as isize;
    let mut y = (SIZE - 1) as isize;
    while x > 0 {
        if x == 6 {
            x -= 1;
        }
        while y >= 0 && y < SIZE as isize {
            for dx in 0..2 {
                let xx = (x - dx) as usize;
                let yy = y as usize;
                if !matrix.reserved[yy][xx] {
                    matrix.modules[yy][xx] = Some(bits.get(bit_index).copied().unwrap_or(false));
                    bit_index += 1;
                }
            }
            y += direction;
        }
        direction = -direction;
        y += direction;
        x -= 2;
    }
}

fn apply_mask_0(matrix: &mut Matrix) {
    for y in 0..SIZE {
        for x in 0..SIZE {
            if !matrix.reserved[y][x] && (x + y) % 2 == 0 {
                matrix.modules[y][x] = Some(!matrix.modules[y][x].unwrap_or(false));
            }
        }
    }
}

fn draw_format_bits(matrix: &mut Matrix) {
    for index in 0..15 {
        let bit = ((FORMAT_L_MASK_0 >> index) & 1) == 1;
        if index < 6 {
            matrix.set(8, index, bit, true);
        } else if index < 8 {
            matrix.set(8, index + 1, bit, true);
        } else {
            matrix.set(8, (SIZE - 15 + index as usize) as isize, bit, true);
        }

        if index < 8 {
            matrix.set((SIZE - 1) as isize - index, 8, bit, true);
        } else if index == 8 {
            matrix.set(7, 8, bit, true);
        } else {
            matrix.set(14 - index, 8, bit, true);
        }
    }
}

fn reed_solomon_remainder(data: &[u8], degree: usize) -> Vec<u8> {
    let generator = reed_solomon_generator(degree);
    let mut result = vec![0u8; degree];
    for byte in data {
        let factor = *byte ^ result[0];
        result.remove(0);
        result.push(0);
        for (index, coefficient) in generator.iter().enumerate() {
            result[index] ^= gf_multiply(*coefficient, factor);
        }
    }
    result
}

fn reed_solomon_generator(degree: usize) -> Vec<u8> {
    let mut result = vec![1u8];
    for exponent in 0..degree {
        let mut next = vec![0u8; result.len() + 1];
        for (index, coefficient) in result.iter().enumerate() {
            next[index] ^= gf_multiply(*coefficient, 1);
            next[index + 1] ^= gf_multiply(*coefficient, gf_pow(2, exponent));
        }
        result = next;
    }
    result.into_iter().skip(1).collect()
}

fn gf_pow(value: u8, exponent: usize) -> u8 {
    let mut result = 1u8;
    for _ in 0..exponent {
        result = gf_multiply(result, value);
    }
    result
}

fn gf_multiply(mut left: u8, mut right: u8) -> u8 {
    let mut result = 0u8;
    while right != 0 {
        if right & 1 != 0 {
            result ^= left;
        }
        let carry = left & 0x80 != 0;
        left <<= 1;
        if carry {
            left ^= 0x1D;
        }
        right >>= 1;
    }
    result
}
