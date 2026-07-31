use std::time::Duration;

// Embed animation frames for each variant at compile time.
macro_rules! frames_for {
    ($dir:literal) => {
        [
            include_str!(concat!("../frames/", $dir, "/frame_1.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_2.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_3.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_4.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_5.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_6.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_7.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_8.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_9.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_10.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_11.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_12.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_13.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_14.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_15.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_16.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_17.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_18.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_19.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_20.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_21.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_22.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_23.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_24.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_25.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_26.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_27.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_28.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_29.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_30.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_31.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_32.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_33.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_34.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_35.txt")),
            include_str!(concat!("../frames/", $dir, "/frame_36.txt")),
        ]
    };
}

pub(crate) const FRAMES_DEFAULT: [&str; 36] = frames_for!("default");
pub(crate) const FRAMES_ODY: [&str; 36] = frames_for!("ody");
pub(crate) const FRAMES_ODYSSEYTHINK: [&str; 36] = frames_for!("odysseythink");
pub(crate) const FRAMES_BLOCKS: [&str; 36] = frames_for!("blocks");
pub(crate) const FRAMES_DOTS: [&str; 36] = frames_for!("dots");
pub(crate) const FRAMES_HASH: [&str; 36] = frames_for!("hash");
pub(crate) const FRAMES_HBARS: [&str; 36] = frames_for!("hbars");
pub(crate) const FRAMES_VBARS: [&str; 36] = frames_for!("vbars");
pub(crate) const FRAMES_SHAPES: [&str; 36] = frames_for!("shapes");
pub(crate) const FRAMES_SLUG: [&str; 36] = frames_for!("slug");

pub(crate) const ALL_VARIANTS: &[&[&str]] = &[
    &FRAMES_DEFAULT,
    &FRAMES_ODY,
    &FRAMES_ODYSSEYTHINK,
    &FRAMES_BLOCKS,
    &FRAMES_DOTS,
    &FRAMES_HASH,
    &FRAMES_HBARS,
    &FRAMES_VBARS,
    &FRAMES_SHAPES,
    &FRAMES_SLUG,
];

pub(crate) const FRAME_TICK_DEFAULT: Duration = Duration::from_millis(80);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_variants_have_expected_frame_count() {
        assert_eq!(ALL_VARIANTS.len(), 10);
        for (idx, variant) in ALL_VARIANTS.iter().enumerate() {
            assert_eq!(
                variant.len(),
                36,
                "variant {idx} must have 36 frames"
            );
        }
    }

    #[test]
    fn all_frames_have_consistent_dimensions() {
        for (v_idx, variant) in ALL_VARIANTS.iter().enumerate() {
            for (f_idx, frame) in variant.iter().enumerate() {
                let lines: Vec<_> = frame.lines().collect();
                assert_eq!(
                    lines.len(),
                    16,
                    "variant {v_idx} frame {f_idx} must be 16 rows"
                );
                for (l_idx, line) in lines.iter().enumerate() {
                    let width = unicode_width::UnicodeWidthStr::width(*line);
                    assert_eq!(
                        width, 80,
                        "variant {v_idx} frame {f_idx} line {l_idx} must be 80 columns"
                    );
                }
            }
        }
    }
}
