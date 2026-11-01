import CoreGraphics

/// Where the picture sits in the stream's layer: centred, with its aspect
/// ratio kept, as the core draws it, so that the pointer and touches map to
/// the pixels they are over.
struct Placement: Equatable {
    var left: CGFloat
    var top: CGFloat
    var width: CGFloat
    var height: CGFloat

    /// Returns where a `video` picture, in pixels, is drawn in a layer of
    /// `layer` pixels, or `nil` if either is empty; the core's
    /// `placement::fit`.
    static func fit(video: CGSize, in layer: CGSize) -> Placement? {
        guard video.width > 0, video.height > 0, layer.width > 0, layer.height > 0 else { return nil }
        let scale = min(layer.width / video.width, layer.height / video.height)
        let width = (video.width * scale).rounded()
        let height = (video.height * scale).rounded()
        return Placement(
            left: ((layer.width - width) / 2).rounded(.down),
            top: ((layer.height - height) / 2).rounded(.down),
            width: width,
            height: height
        )
    }

    /// Returns the point of the layer `point`, in pixels from its top left,
    /// in the video's pixels, clamped to the picture.
    func videoPoint(_ point: CGPoint, video: CGSize) -> CGPoint {
        let x = (point.x - left) * video.width / width
        let y = (point.y - top) * video.height / height
        return CGPoint(
            x: min(max(x, 0), video.width - 1),
            y: min(max(y, 0), video.height - 1)
        )
    }
}
