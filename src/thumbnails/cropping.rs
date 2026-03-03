/// Given original image size and target thumbnail size, finds subimage x, y, width, height in the
/// original image, so that the cropped image is centered, maximally sized and has identical aspect
/// ratio to target_size. The output crop is also always non-empty.
pub fn crop_coordinates(
    orig_size: (u32, u32),
    target_size: (u32, u32),
    vertical_bias_percent: u32,
) -> (u32, u32, u32, u32) {
    debug_assert!(
        orig_size.0 > 0 && orig_size.1 > 0,
        "orig_size dimensions must be non-zero"
    );
    debug_assert!(
        target_size.0 > 0 && target_size.1 > 0,
        "target_size dimensions must be non-zero"
    );

    let ow = orig_size.0 as u64;
    let oh = orig_size.1 as u64;
    let tw = target_size.0 as u64;
    let th = target_size.1 as u64;

    if ow * th > tw * oh {
        // Original is wider than target
        let height = orig_size.1;
        let width = ((tw * oh + th / 2) / th).max(1) as u32;
        let x = (orig_size.0 - width) / 2;
        (x, 0, width, height)
    } else {
        // Original is narrower than target
        let width = orig_size.0;
        let height = ((th * ow + tw / 2) / tw).max(1) as u32;
        let y = ((orig_size.1 - height) as u64 * vertical_bias_percent as u64 / 100) as u32;
        (0, y, width, height)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert2::assert;
    use proptest::prop_assume;
    use test_strategy::proptest;

    #[test]
    fn crop_coordinates_example() {
        assert!(crop_coordinates((200, 100), (50, 50), 50) == (50, 0, 100, 100));
    }

    #[proptest]
    fn crop_coordinates_all(
        orig_size: (u32, u32),
        target_size: (u32, u32),
        #[strategy(0u32..=100u32)] vertical_bias_percent: u32,
    ) {
        prop_assume!(orig_size.0 > 0);
        prop_assume!(orig_size.1 > 0);
        prop_assume!(target_size.0 > 0);
        prop_assume!(target_size.1 > 0);

        let (x, y, w, h) = crop_coordinates(orig_size, target_size, vertical_bias_percent);

        assert!(w > 0);
        assert!(h > 0);

        // We're staying in bounds:
        assert!(x + w <= orig_size.0);
        assert!(y + h <= orig_size.1);

        // The output is maximum sized.
        assert!(w == orig_size.0 || h == orig_size.1);

        // x is centered in input, +- 1 pixel
        let gap = (orig_size.0 - w) as u64;
        let x = x as u64;
        assert!(gap + 1 >= 2 * x);
        assert!(gap <= 2 * x + 1);

        // Target aspect ratio is kept
        //assert!((h as u64) * (target_size.0 as u64) / (target_size.1 as u64) + 1 >= (w as u64)); // TODO: Rounding!
        //assert!((w as u64) * (target_size.1 as u64) / (target_size.0 as u64) + 1 >= (h as u64)); // TODO: Rounding!
    }

    #[proptest]
    fn crop_coordinates_50_bias(orig_size: (u32, u32), target_size: (u32, u32)) {
        prop_assume!(orig_size.0 > 0);
        prop_assume!(orig_size.1 > 0);
        prop_assume!(target_size.0 > 0);
        prop_assume!(target_size.1 > 0);

        let (_x, y, _w, h) = crop_coordinates(orig_size, target_size, 50);

        // y is centered in input, +- 1 pixel
        let gap = (orig_size.1 - h) as u64;
        let y = y as u64;
        assert!(gap + 1 >= 2 * y);
        assert!(gap <= 2 * y + 1);
    }

    #[proptest]
    fn crop_coordinates_zero_bias(orig_size: (u32, u32), target_size: (u32, u32)) {
        prop_assume!(orig_size.0 > 0);
        prop_assume!(orig_size.1 > 0);
        prop_assume!(target_size.0 > 0);
        prop_assume!(target_size.1 > 0);

        let (_x, y, _w, _h) = crop_coordinates(orig_size, target_size, 0);

        assert!(y == 0);
    }
}
