//! The parser checked against rav1d: streams rav1e encodes with different
//! coding tools are read by both, and every field rav1d reports must match.

use rav1d::include::dav1d::headers::Dav1dFrameHeader;
use rav1e::Context;
use rav1e::config::{Config, EncoderConfig, GrainTableSegment, SpeedSettings};
use rav1e::data::EncoderStatus;
use rav1e::prelude::SceneDetectionSpeed;

use super::*;
use crate::Av1Decoder;

/// The frames each stream has.
const FRAMES: usize = 24;

/// The quantizer of the test streams: low enough that rav1e uses its coding
/// tools rather than skipping every block.
const QUANTIZER: usize = 80;

/// dav1d's `lr_type` for each [`RestorationType`]: the coded value, before
/// `Remap_Lr_Type`.
fn coded_restoration_type(kind: RestorationType) -> u8 {
    match kind {
        RestorationType::None => 0,
        RestorationType::Switchable => 1,
        RestorationType::Wiener => 2,
        RestorationType::SelfGuided => 3,
    }
}

/// Fills frame `index` of a moving scene: a gradient that scrolls, a
/// checkerboard that moves the other way, and noise, so that rav1e predicts
/// across frames and uses its filters.
fn scene(width: usize, height: usize, index: usize) -> [Vec<u8>; 3] {
    let mut noise = 0x9e37_79b9u32.wrapping_mul(index as u32 + 1);
    let mut next = || {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        (noise >> 28) as u8
    };
    let luma = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let gradient = ((x + 3 * index) * 255 / width) as u8;
            let checker = if ((x + width - 2 * index % width) / 16 + y / 16).is_multiple_of(2) {
                40
            } else {
                0
            };
            gradient.wrapping_add(checker).wrapping_add(next())
        })
        .collect();
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    let chroma = |offset: usize| {
        (0..chroma_width * chroma_height)
            .map(|i| (128 + ((i % chroma_width + offset + index) % 64)) as u8)
            .collect()
    };
    [luma, chroma(0), chroma(32)]
}

/// Encodes [`FRAMES`] frames of the scene with `config` and returns the
/// temporal units.
fn encode(config: EncoderConfig) -> Vec<Vec<u8>> {
    let (width, height) = (config.width, config.height);
    let frames = if config.still_picture { 1 } else { FRAMES };
    let mut context: Context<u8> = Config::new()
        .with_encoder_config(config)
        .with_threads(1)
        .new_context()
        .unwrap();
    let mut units = Vec::new();
    let mut receive = |context: &mut Context<u8>| loop {
        match context.receive_packet() {
            Ok(packet) => units.push(packet.data),
            Err(EncoderStatus::Encoded) => {}
            Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => return,
            Err(error) => panic!("rav1e failed: {error}"),
        }
    };
    for index in 0..frames {
        let planes = scene(width, height, index);
        let mut frame = context.new_frame();
        frame.planes[0].copy_from_raw_u8(&planes[0], width, 1);
        frame.planes[1].copy_from_raw_u8(&planes[1], width.div_ceil(2), 1);
        frame.planes[2].copy_from_raw_u8(&planes[2], width.div_ceil(2), 1);
        context.send_frame(frame).unwrap();
        receive(&mut context);
    }
    context.flush();
    receive(&mut context);
    assert_eq!(units.len(), frames, "every frame was encoded");
    units
}

/// The configuration the tests start from: `width × height`, rav1e's
/// `speed`, its low-latency mode as QDP's encoder uses it, and `tiles`.
fn base_config(width: usize, height: usize, speed: u8, tiles: usize) -> EncoderConfig {
    let mut speed_settings = SpeedSettings::from_preset(speed);
    speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
    EncoderConfig {
        width,
        height,
        low_latency: true,
        min_key_frame_interval: 0,
        max_key_frame_interval: 12,
        quantizer: QUANTIZER,
        tiles,
        speed_settings,
        ..EncoderConfig::default()
    }
}

/// Parses every temporal unit and checks the frame each one shows against
/// rav1d's reading of it, returning the frames parsed.
fn check_stream(units: &[Vec<u8>]) -> Vec<ParsedFrame> {
    let mut parser = FrameParser::new();
    let mut decoder = Av1Decoder::new(1).unwrap();
    let mut all = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        let frames = parser.parse(unit).unwrap_or_else(|error| {
            panic!("temporal unit {index} did not parse: {error}");
        });
        let theirs = decoder.decode_frame_header(unit).unwrap();
        let shown = frames
            .iter()
            .rfind(|frame| frame.header.show_frame || frame.header.show_existing_frame)
            .expect("every temporal unit shows a frame");
        if shown.header.show_existing_frame {
            // rav1d returns the header of the frame shown again.
            assert_eq!(shown.header.order_hint, u32::from(theirs.frame_offset));
            assert_eq!(shown.header.size.upscaled_width as i32, theirs.width[1]);
        } else {
            check_frame(index, shown, &theirs);
        }
        all.extend(frames);
    }
    all
}

/// Checks a parsed frame against rav1d's header of it.
fn check_frame(index: usize, ours: &ParsedFrame, theirs: &Dav1dFrameHeader) {
    let header = &ours.header;
    let context = format!("temporal unit {index}");
    let flag = |value: bool| u8::from(value);
    assert_eq!(
        header.frame_type as u32, theirs.frame_type,
        "{context}: frame_type"
    );
    assert_eq!(
        [header.size.width as i32, header.size.upscaled_width as i32],
        theirs.width,
        "{context}: width"
    );
    assert_eq!(
        header.size.height as i32, theirs.height,
        "{context}: height"
    );
    assert_eq!(
        [
            header.size.render_width as i32,
            header.size.render_height as i32
        ],
        [theirs.render_width, theirs.render_height],
        "{context}: render size"
    );
    assert_eq!(
        header.order_hint,
        u32::from(theirs.frame_offset),
        "{context}: order hint"
    );
    assert_eq!(
        [
            flag(header.show_frame),
            flag(header.showable_frame),
            flag(header.error_resilient_mode),
            flag(header.disable_cdf_update),
            flag(header.allow_screen_content_tools),
            flag(header.force_integer_mv),
            flag(header.allow_intrabc),
            flag(!header.disable_frame_end_update_cdf),
        ],
        [
            theirs.show_frame,
            theirs.showable_frame,
            theirs.error_resilient_mode,
            theirs.disable_cdf_update,
            theirs.allow_screen_content_tools,
            theirs.force_integer_mv,
            theirs.allow_intrabc,
            theirs.refresh_context,
        ],
        "{context}: frame flags"
    );
    assert_eq!(
        header.primary_ref_frame, theirs.primary_ref_frame,
        "{context}: primary ref"
    );
    assert_eq!(
        header.refresh_frame_flags, theirs.refresh_frame_flags,
        "{context}: refresh"
    );
    assert_eq!(
        header.current_frame_id, theirs.frame_id,
        "{context}: frame id"
    );
    if !header.frame_type.is_intra() {
        assert_eq!(
            header.ref_frame_idx.map(|slot| slot as i8),
            theirs.refidx,
            "{context}: references"
        );
        assert_eq!(
            [
                flag(header.allow_high_precision_mv),
                flag(header.is_motion_mode_switchable),
                flag(header.use_ref_frame_mvs),
            ],
            [
                theirs.hp,
                theirs.switchable_motion_mode,
                theirs.use_ref_frame_mvs
            ],
            "{context}: inter flags"
        );
        assert_eq!(
            header.interpolation_filter, theirs.subpel_filter_mode,
            "{context}: interpolation filter"
        );
    }
    check_tiles(&context, ours, theirs);
    check_quantization(&context, header, theirs);
    check_filters(&context, header, theirs);
    assert_eq!(
        header.tx_mode as u32, theirs.txfm_mode,
        "{context}: tx mode"
    );
    assert_eq!(
        [
            flag(header.reference_select),
            flag(header.skip_mode_present),
            flag(header.allow_warped_motion),
            flag(header.reduced_tx_set),
            flag(header.all_lossless),
        ],
        [
            theirs.switchable_comp_refs,
            theirs.skip_mode_enabled,
            theirs.warp_motion,
            theirs.reduced_txtp_set,
            theirs.all_lossless,
        ],
        "{context}: mode flags"
    );
    for (reference, theirs) in theirs.gmv.iter().enumerate() {
        let motion = &header.global_motion[reference + 1];
        assert_eq!(
            motion.model as u32, theirs.r#type,
            "{context}: global motion type"
        );
        assert_eq!(motion.params, theirs.matrix, "{context}: global motion");
    }
    check_film_grain(&context, header, theirs);
}

/// Checks the tile layout, and that the tiles fill each tile group.
fn check_tiles(context: &str, ours: &ParsedFrame, theirs: &Dav1dFrameHeader) {
    let tiles = &ours.header.tiles;
    let tiling = &theirs.tiling;
    assert_eq!(
        [tiles.cols(), tiles.rows()],
        [usize::from(tiling.cols), usize::from(tiling.rows)],
        "{context}: tiles"
    );
    assert_eq!(
        [tiles.cols_log2, tiles.rows_log2],
        [u32::from(tiling.log2_cols), u32::from(tiling.log2_rows)],
        "{context}: tile log2"
    );
    let starts = |sizes: &[u32]| -> Vec<u16> {
        sizes
            .iter()
            .scan(0u16, |start, &size| {
                let this = *start;
                *start += size as u16;
                Some(this)
            })
            .collect()
    };
    assert_eq!(
        starts(&tiles.col_widths)[..],
        tiling.col_start_sb[..tiles.cols()],
        "{context}: tile columns"
    );
    assert_eq!(
        starts(&tiles.row_heights)[..],
        tiling.row_start_sb[..tiles.rows()],
        "{context}: tile rows"
    );
    if tiles.count() > 1 {
        assert_eq!(tiles.context_update_tile_id, u32::from(tiling.update));
        assert_eq!(tiles.tile_size_bytes, u32::from(tiling.n_bytes));
    }
    assert_eq!(
        ours.tiles.len(),
        tiles.count(),
        "{context}: every tile found"
    );
    for range in &ours.tile_data {
        let inside: usize = ours
            .tiles
            .iter()
            .filter(|tile| range.contains(&tile.offset))
            .map(|tile| tile.size)
            .sum();
        let sizes = (inside..=range.len()).contains(&range.len());
        assert!(sizes, "{context}: tiles lie in their group");
    }
    let last = ours.tiles.last().unwrap();
    assert_eq!(
        last.offset + last.size,
        ours.tile_data.last().unwrap().end,
        "{context}: the last tile ends its group"
    );
}

/// Checks the quantizers, segmentation and deltas.
fn check_quantization(context: &str, header: &FrameHeader, theirs: &Dav1dFrameHeader) {
    let quantization = &header.quantization;
    let quant = &theirs.quant;
    assert_eq!(
        [
            quantization.delta_q_y_dc,
            quantization.delta_q_u_dc,
            quantization.delta_q_u_ac,
            quantization.delta_q_v_dc,
            quantization.delta_q_v_ac,
        ],
        [
            quant.ydc_delta,
            quant.udc_delta,
            quant.uac_delta,
            quant.vdc_delta,
            quant.vac_delta,
        ],
        "{context}: quantizer deltas"
    );
    assert_eq!(quantization.base_q_idx, quant.yac, "{context}: base_q_idx");
    let matrices = quantization.matrices.unwrap_or_default();
    assert_eq!(
        [
            u8::from(quantization.matrices.is_some()),
            matrices.y,
            matrices.u,
            matrices.v,
        ],
        [quant.qm, quant.qm_y, quant.qm_u, quant.qm_v],
        "{context}: quantizer matrices"
    );
    let segmentation = &header.segmentation;
    let theirs_segmentation = &theirs.segmentation;
    assert_eq!(
        [
            u8::from(segmentation.enabled),
            u8::from(segmentation.update_map),
            u8::from(segmentation.temporal_update),
            u8::from(segmentation.update_data),
        ],
        [
            theirs_segmentation.enabled,
            theirs_segmentation.update_map,
            theirs_segmentation.temporal,
            theirs_segmentation.update_data,
        ],
        "{context}: segmentation"
    );
    if segmentation.enabled {
        let features = &segmentation.features;
        for (segment, data) in theirs_segmentation.seg_data.d.iter().enumerate() {
            let value = |feature: usize| {
                if features.is_enabled(segment, feature) {
                    features.data[segment][feature]
                } else {
                    0
                }
            };
            let reference = if features.is_enabled(segment, 5) {
                features.data[segment][5] as i8
            } else {
                -1
            };
            assert_eq!(
                [
                    value(0),
                    value(1),
                    value(2),
                    value(3),
                    value(4),
                    i16::from(reference),
                    i16::from(features.is_enabled(segment, 6)),
                    i16::from(features.is_enabled(segment, 7)),
                ],
                [
                    data.delta_q,
                    data.delta_lf_y_v.into(),
                    data.delta_lf_y_h.into(),
                    data.delta_lf_u.into(),
                    data.delta_lf_v.into(),
                    data.r#ref.into(),
                    data.skip.into(),
                    data.globalmv.into(),
                ],
                "{context}: segment {segment}"
            );
        }
    }
    let delta = &header.delta;
    assert_eq!(
        [
            u8::from(delta.q_present),
            delta.q_res,
            u8::from(delta.lf_present),
            delta.lf_res,
            u8::from(delta.lf_multi),
        ],
        [
            theirs.delta.q.present,
            theirs.delta.q.res_log2,
            theirs.delta.lf.present,
            theirs.delta.lf.res_log2,
            theirs.delta.lf.multi,
        ],
        "{context}: deltas"
    );
}

/// Checks the loop filter, CDEF and loop restoration.
fn check_filters(context: &str, header: &FrameHeader, theirs: &Dav1dFrameHeader) {
    let filter = &header.loop_filter;
    let theirs_filter = &theirs.loopfilter;
    assert_eq!(
        filter.levels,
        [
            theirs_filter.level_y[0],
            theirs_filter.level_y[1],
            theirs_filter.level_u,
            theirs_filter.level_v,
        ],
        "{context}: loop filter levels"
    );
    assert_eq!(
        [
            filter.sharpness,
            u8::from(filter.delta_enabled),
            u8::from(filter.delta_update),
        ],
        [
            theirs_filter.sharpness,
            theirs_filter.mode_ref_delta_enabled,
            theirs_filter.mode_ref_delta_update,
        ],
        "{context}: loop filter"
    );
    assert_eq!(
        (filter.ref_deltas, filter.mode_deltas),
        (
            theirs_filter.mode_ref_deltas.ref_delta,
            theirs_filter.mode_ref_deltas.mode_delta
        ),
        "{context}: loop filter deltas"
    );
    let cdef = &header.cdef;
    assert_eq!(
        [cdef.damping_minus_3 + 3, cdef.bits],
        [theirs.cdef.damping, theirs.cdef.n_bits],
        "{context}: CDEF"
    );
    let strengths = 1 << cdef.bits;
    for index in 0..strengths {
        assert_eq!(
            [
                cdef.y_primary[index] << 2 | cdef.y_secondary[index],
                cdef.uv_primary[index] << 2 | cdef.uv_secondary[index],
            ],
            [
                theirs.cdef.y_strength[index],
                theirs.cdef.uv_strength[index]
            ],
            "{context}: CDEF strength {index}"
        );
    }
    let restoration = &header.restoration;
    assert_eq!(
        restoration.types.map(coded_restoration_type),
        theirs.restoration.r#type,
        "{context}: restoration types"
    );
    if restoration.is_used() {
        assert_eq!(
            [restoration.unit_size_log2[0], restoration.unit_size_log2[1]],
            theirs.restoration.unit_size,
            "{context}: restoration units"
        );
    }
}

/// Checks the film grain.
fn check_film_grain(context: &str, header: &FrameHeader, theirs: &Dav1dFrameHeader) {
    let grain = &header.film_grain;
    let theirs_grain = &theirs.film_grain;
    assert_eq!(
        u8::from(grain.apply_grain),
        theirs_grain.present,
        "{context}: film grain"
    );
    if !grain.apply_grain {
        return;
    }
    let data = &theirs_grain.data;
    assert_eq!(
        u8::from(grain.update_grain),
        theirs_grain.update,
        "{context}: update grain"
    );
    assert_eq!(
        u32::from(grain.grain_seed),
        data.seed,
        "{context}: grain seed"
    );
    assert_eq!(
        grain.y_points.len() as i32,
        data.num_y_points,
        "{context}: luma points"
    );
    assert_eq!(grain.y_points[..], data.y_points[..grain.y_points.len()]);
    assert_eq!(
        [grain.cb_points.len() as i32, grain.cr_points.len() as i32],
        data.num_uv_points,
        "{context}: chroma points"
    );
    assert_eq!(
        grain.cb_points[..],
        data.uv_points[0][..grain.cb_points.len()]
    );
    assert_eq!(
        grain.cr_points[..],
        data.uv_points[1][..grain.cr_points.len()]
    );
    assert_eq!(
        [
            i32::from(grain.chroma_scaling_from_luma),
            i32::from(grain.scaling_shift_minus_8) + 8,
            i32::from(grain.ar_coeff_lag),
            i32::from(grain.grain_scale_shift),
            i32::from(grain.overlap),
            i32::from(grain.clip_to_restricted_range),
        ],
        [
            data.chroma_scaling_from_luma,
            data.scaling_shift,
            data.ar_coeff_lag,
            data.grain_scale_shift,
            data.overlap_flag,
            data.clip_to_restricted_range,
        ],
        "{context}: grain parameters"
    );
    assert_eq!(
        u64::from(grain.ar_coeff_shift_minus_6) + 6,
        data.ar_coeff_shift,
        "{context}: AR shift"
    );
    let signed = |coefficients: &[u8]| -> Vec<i8> {
        coefficients
            .iter()
            .map(|&coefficient| coefficient.wrapping_sub(128) as i8)
            .collect()
    };
    let lag = usize::from(grain.ar_coeff_lag);
    let luma = 2 * lag * (lag + 1);
    let chroma = luma + usize::from(!grain.y_points.is_empty());
    if !grain.y_points.is_empty() {
        assert_eq!(
            signed(&grain.ar_coeffs_y[..luma])[..],
            data.ar_coeffs_y[..luma],
            "{context}: luma AR"
        );
    }
    if grain.chroma_scaling_from_luma || !grain.cb_points.is_empty() {
        assert_eq!(
            signed(&grain.ar_coeffs_cb[..chroma])[..],
            data.ar_coeffs_uv[0][..chroma],
            "{context}: Cb AR"
        );
    }
    if grain.chroma_scaling_from_luma || !grain.cr_points.is_empty() {
        assert_eq!(
            signed(&grain.ar_coeffs_cr[..chroma])[..],
            data.ar_coeffs_uv[1][..chroma],
            "{context}: Cr AR"
        );
    }
    if !grain.cb_points.is_empty() {
        assert_eq!(
            [
                i32::from(grain.cb_mult) - 128,
                i32::from(grain.cb_luma_mult) - 128,
                i32::from(grain.cb_offset) - 256,
            ],
            [data.uv_mult[0], data.uv_luma_mult[0], data.uv_offset[0]],
            "{context}: Cb grain"
        );
    }
    if !grain.cr_points.is_empty() {
        assert_eq!(
            [
                i32::from(grain.cr_mult) - 128,
                i32::from(grain.cr_luma_mult) - 128,
                i32::from(grain.cr_offset) - 256,
            ],
            [data.uv_mult[1], data.uv_luma_mult[1], data.uv_offset[1]],
            "{context}: Cr grain"
        );
    }
}

/// QDP's own encoder: low latency, the fastest preset, two tiles at 720p.
#[test]
fn reads_qdp_streams() {
    let mut encoder = crate::Av1Encoder::new(crate::EncoderSettings {
        width: 1280,
        height: 720,
        frame_rate: 60,
        speed: crate::FASTEST_SPEED,
        quantizer: QUANTIZER as u8,
        threads: 1,
    })
    .unwrap();
    let mut units = Vec::new();
    for index in 0..FRAMES {
        let [y, u, v] = scene(1280, 720, index);
        let frame = crate::Yuv420Frame::from_planes(1280, 720, y, u, v).unwrap();
        let request = if index == FRAMES / 2 {
            crate::KeyFrameRequest::Force
        } else {
            crate::KeyFrameRequest::Auto
        };
        units.extend(
            encoder
                .encode(&frame, request)
                .unwrap()
                .into_iter()
                .map(|encoded| encoded.temporal_unit),
        );
    }
    let frames = check_stream(&units);
    assert!(frames.iter().any(|frame| frame.header.tiles.count() > 1));
}

/// A slower preset, which uses more of rav1e's tools: compound prediction,
/// loop restoration, segmentation and many tiles.
#[test]
fn reads_slower_presets() {
    let frames = check_stream(&encode(base_config(640, 360, 4, 8)));
    assert!(frames.iter().any(|frame| frame.header.tiles.count() > 1));
}

/// Frames out of order: hidden alternate references, shown again later
/// with `show_existing_frame`, and skip mode between them.
#[test]
fn reads_reordered_streams() {
    let config = EncoderConfig {
        low_latency: false,
        ..base_config(352, 288, 6, 1)
    };
    let frames = check_stream(&encode(config));
    assert!(frames.iter().any(|frame| !frame.header.show_frame));
    assert!(frames.iter().any(|frame| frame.header.show_existing_frame));
    assert!(frames.iter().any(|frame| frame.header.reference_select));
}

/// Error resilient frames and switch frames, which carry the order hints of
/// every reference.
#[test]
fn reads_error_resilient_streams() {
    let config = EncoderConfig {
        error_resilient: true,
        switch_frame_interval: 5,
        ..base_config(320, 240, 8, 1)
    };
    let frames = check_stream(&encode(config));
    assert!(
        frames
            .iter()
            .any(|frame| frame.header.frame_type == FrameType::Switch)
    );
}

/// A still picture, whose sequence header is reduced.
#[test]
fn reads_still_pictures() {
    let config = EncoderConfig {
        still_picture: true,
        ..base_config(256, 256, 8, 1)
    };
    let units = encode(config);
    let frames = check_stream(&units);
    assert_eq!(frames.len(), 1);
}

/// Film grain, coded with key frames and reused by inter frames.
#[test]
fn reads_film_grain() {
    let mut grain = av1_grain::generate_photon_noise_params(
        0,
        u64::MAX,
        av1_grain::NoiseGenArgs {
            iso_setting: 1600,
            width: 320,
            height: 240,
            transfer_function: av1_grain::TransferFunction::BT1886,
            chroma_grain: true,
            random_seed: None,
        },
    );
    // A lag of 1 has 4 luma coefficients, and 5 for each chroma plane.
    grain.ar_coeff_lag = 1;
    for coefficient in [3, -2, 5, -7] {
        grain.ar_coeffs_y.push(coefficient);
    }
    for coefficient in [1, -1, 2, -2, 4] {
        grain.ar_coeffs_cb.push(coefficient);
        grain.ar_coeffs_cr.push(-coefficient);
    }
    grain.scaling_points_cb.push([0, 20]);
    grain.scaling_points_cb.push([255, 40]);
    grain.scaling_points_cr.push([64, 30]);
    (grain.cb_mult, grain.cb_luma_mult, grain.cb_offset) = (140, 120, 300);
    (grain.cr_mult, grain.cr_luma_mult, grain.cr_offset) = (110, 150, 200);
    let segment: GrainTableSegment = grain;
    let config = EncoderConfig {
        film_grain_params: Some(vec![segment]),
        ..base_config(320, 240, 8, 1)
    };
    let frames = check_stream(&encode(config));
    assert!(
        frames
            .iter()
            .all(|frame| frame.header.film_grain.apply_grain)
    );
}

/// A corrupt header is refused rather than read past its end.
#[test]
fn refuses_truncated_headers() {
    let units = encode(base_config(128, 128, 10, 1));
    let mut parser = FrameParser::new();
    parser.parse(&units[0]).unwrap();
    // The second unit, cut inside its frame header: an OBU_FRAME of three
    // bytes.
    let truncated = [0x32, 0x03, 0x10, 0x00, 0x00];
    assert!(parser.parse(&truncated).is_err());
    assert!(
        FrameParser::new().parse(&units[1]).is_err(),
        "no sequence header"
    );
}

/// The headers QDP's VA-API encoder writes itself parse to what it meant:
/// a key frame, then an inter frame predicted from one slot, each in two
/// tiles.
#[cfg(target_os = "linux")]
#[test]
fn reads_vaapi_headers() {
    use crate::headers::{self, FrameKind, StreamFormat, TileLayout};

    /// The bytes of each tile's size field.
    const TILE_SIZE_BYTES: u8 = 4;

    /// Returns a tile group OBU for two tiles holding `first` and `second`.
    fn tile_group(first: &[u8], second: &[u8]) -> Vec<u8> {
        // tile_start_and_end_present_flag 0, then byte alignment.
        let mut payload = vec![0];
        payload.extend_from_slice(&(first.len() as u32 - 1).to_le_bytes());
        payload.extend_from_slice(first);
        payload.extend_from_slice(second);
        let mut obu = vec![0x22];
        crate::bits::write_leb128(&mut obu, payload.len() as u64);
        obu.extend(payload);
        obu
    }

    let tiles = TileLayout::new(1920, 1080, 2);
    let format = StreamFormat {
        width: 1920,
        height: 1080,
        level: crate::level_for(1920, 1080, 60, tiles.count_and_columns()),
    };
    let frame = |kind, order_hint| headers::FrameHeader {
        kind,
        order_hint,
        base_q_idx: 120,
        loop_filter_level: 20,
        tx_mode: headers::TxMode::Select,
        tiles,
        tile_size_bytes: TILE_SIZE_BYTES,
    };
    let key = [
        headers::sequence_header_obu(&format),
        headers::frame_header_obu(&format, &frame(FrameKind::Key, 0)),
        tile_group(&[1, 2, 3], &[4, 5]),
    ]
    .concat();
    let inter_kind = FrameKind::Inter {
        reference_slot: 0,
        refresh_frame_flags: 1 << 2,
    };
    let inter = [
        headers::frame_header_obu(&format, &frame(inter_kind, 1)),
        tile_group(&[6], &[7, 8, 9, 10]),
    ]
    .concat();

    let mut parser = FrameParser::new();
    let [key_frame] = parser.parse(&key).unwrap().try_into().unwrap();
    let header = &key_frame.header;
    assert_eq!(header.frame_type, FrameType::Key);
    assert_eq!((header.size.width, header.size.height), (1920, 1080));
    assert_eq!(header.quantization.base_q_idx, 120);
    assert_eq!(header.loop_filter.levels, [20; 4]);
    assert_eq!(header.tx_mode, header::TxMode::Select);
    assert_eq!((header.tiles.cols(), header.tiles.rows()), (2, 1));
    assert_eq!(header.tiles.tile_size_bytes, u32::from(TILE_SIZE_BYTES));
    assert_eq!(header.refresh_frame_flags, header::ALL_FRAMES);
    let sizes: Vec<_> = key_frame
        .tiles
        .iter()
        .map(|tile| (&key[tile.offset..tile.offset + tile.size], tile.column))
        .collect();
    assert_eq!(sizes, [(&[1u8, 2, 3][..], 0), (&[4, 5][..], 1)]);

    let [inter_frame] = parser.parse(&inter).unwrap().try_into().unwrap();
    let header = &inter_frame.header;
    assert_eq!(header.frame_type, FrameType::Inter);
    assert_eq!(header.order_hint, 1);
    assert_eq!(header.ref_frame_idx, [0; REFS_PER_FRAME]);
    assert_eq!(header.refresh_frame_flags, 1 << 2);
    assert_eq!(header.primary_ref_frame, 0);
    assert_eq!(inter_frame.references[0].width, 1920);
    let last = inter_frame.tiles[1];
    assert_eq!(&inter[last.offset..last.offset + last.size], [7, 8, 9, 10]);
    assert!(parser.refs()[2].valid && parser.refs()[2].order_hint == 1);
}
