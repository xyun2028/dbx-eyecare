//! 色温到 Gamma 查找表的换算。
//!
//! 和主项目 eyecare 用的是同一套算法：Tanner Helland 的色温拟合 +
//! 峰值归一化。这里不依赖任何系统接口，方便单测。

/// 色温转 RGB 白点，各通道 0.0 ~ 1.0。
pub fn kelvin_to_rgb(kelvin: u32) -> (f64, f64, f64) {
    let t = kelvin.clamp(1000, 40000) as f64 / 100.0;

    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };

    let g = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };

    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };

    let norm = |v: f64| (v / 255.0).clamp(0.0, 1.0);
    (norm(r), norm(g), norm(b))
}

/// 生成 Gamma 表：768 个 u16，前 256 红、中 256 绿、后 256 蓝。
///
/// 三个通道按最大值归一化，保证最亮那一路仍然是 1.0，
/// 否则调色温会连带把整块屏幕压暗，用户会以为是亮度变了。
pub fn build_gamma_ramp(kelvin: u32, brightness: f64) -> [u16; 768] {
    let (r, g, b) = kelvin_to_rgb(kelvin);
    let peak = r.max(g).max(b).max(f64::MIN_POSITIVE);

    let mut scale = [r / peak, g / peak, b / peak];
    for s in scale.iter_mut() {
        *s = (*s * brightness).clamp(0.0, 1.0);
    }

    let mut ramp = [0u16; 768];
    for i in 0..256usize {
        let input = i as f64 / 255.0;
        for (ch, s) in scale.iter().enumerate() {
            ramp[ch * 256 + i] = (input * s * 65535.0).round().clamp(0.0, 65535.0) as u16;
        }
    }
    ramp
}

/// 按自选颜色生成 Gamma 表。这是「滤色」，不是色温：
/// 三个通道一起按比例压，画面会变暗，这是滤色的本质。
pub fn build_gamma_ramp_rgb(rgb: [u8; 3], brightness: f64) -> [u16; 768] {
    let mut ramp = [0u16; 768];
    for (ch, value) in rgb.iter().enumerate() {
        let scale = (*value as f64 / 255.0 * brightness).clamp(0.0, 1.0);
        for i in 0..256usize {
            let input = i as f64 / 255.0;
            ramp[ch * 256 + i] = (input * scale * 65535.0).round().clamp(0.0, 65535.0) as u16;
        }
    }
    ramp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 标准白光接近原样() {
        let ramp = build_gamma_ramp(6500, 1.0);
        for i in 0..256 {
            let identity = (i as f64 / 255.0 * 65535.0).round() as i32;
            for ch in 0..3 {
                let diff = (ramp[ch * 256 + i] as i32 - identity).abs();
                assert!(diff < 1500, "偏差过大：{} vs {}", ramp[ch * 256 + i], identity);
            }
        }
    }

    #[test]
    fn 低色温蓝色被压低且曲线单调() {
        let ramp = build_gamma_ramp(3400, 1.0);
        assert!(ramp[512 + 255] < ramp[255], "3400K 时蓝色峰值应低于红色");
        for ch in 0..3 {
            for i in 1..256 {
                assert!(
                    ramp[ch * 256 + i] >= ramp[ch * 256 + i - 1],
                    "通道 {ch} 在 {i} 处回落，曲线会断档"
                );
            }
        }
    }

    #[test]
    fn 滤色按比例压低各通道() {
        let ramp = build_gamma_ramp_rgb([199, 237, 204], 1.0);
        assert_eq!(ramp[255], 51143);
        assert_eq!(ramp[256 + 255], 60909);
        assert_eq!(ramp[512 + 255], 52428);
    }
}
