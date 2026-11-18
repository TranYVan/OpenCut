// Writes video.mp4: 2 s of 64x48 H.264 at 30 fps (a frame counter drawn as
// a moving bar) with a 440 Hz AAC stereo tone at 48 kHz. Run on macOS:
//   swiftc make-video.swift -o /tmp/make-video && /tmp/make-video video.mp4
import AVFoundation
import CoreVideo

let output = URL(fileURLWithPath: CommandLine.arguments[1])
try? FileManager.default.removeItem(at: output)

let width = 64, height = 48, fps: Int32 = 30, frames = 60
let sampleRate = 48_000.0, channels = 2

let writer = try AVAssetWriter(outputURL: output, fileType: .mp4)

let video = AVAssetWriterInput(mediaType: .video, outputSettings: [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
    AVVideoCompressionPropertiesKey: [AVVideoMaxKeyFrameIntervalKey: 15],
])
video.expectsMediaDataInRealTime = false
let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
    kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
    kCVPixelBufferWidthKey as String: width,
    kCVPixelBufferHeightKey as String: height,
])
writer.add(video)

let audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
    AVFormatIDKey: kAudioFormatMPEG4AAC,
    AVSampleRateKey: sampleRate,
    AVNumberOfChannelsKey: channels,
    AVEncoderBitRateKey: 64_000,
])
audio.expectsMediaDataInRealTime = false
writer.add(audio)

writer.startWriting()
writer.startSession(atSourceTime: .zero)

var format = AudioStreamBasicDescription(
    mSampleRate: sampleRate, mFormatID: kAudioFormatLinearPCM,
    mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
    mBytesPerPacket: UInt32(4 * channels), mFramesPerPacket: 1,
    mBytesPerFrame: UInt32(4 * channels), mChannelsPerFrame: UInt32(channels),
    mBitsPerChannel: 32, mReserved: 0)
var description: CMAudioFormatDescription?
CMAudioFormatDescriptionCreate(allocator: nil, asbd: &format, layoutSize: 0, layout: nil,
    magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &description)

func appendAudio(from start: Int, count: Int) {
    var samples = [Float](repeating: 0, count: count * channels)
    for i in 0..<count {
        let value = Float(0.5 * sin(2 * Double.pi * 440 * Double(start + i) / sampleRate))
        for c in 0..<channels { samples[i * channels + c] = value }
    }
    var block: CMBlockBuffer?
    let bytes = samples.count * 4
    CMBlockBufferCreateWithMemoryBlock(allocator: nil, memoryBlock: nil, blockLength: bytes,
        blockAllocator: nil, customBlockSource: nil, offsetToData: 0, dataLength: bytes,
        flags: 0, blockBufferOut: &block)
    samples.withUnsafeBytes { raw in
        _ = CMBlockBufferReplaceDataBytes(with: raw.baseAddress!, blockBuffer: block!, offsetIntoDestination: 0, dataLength: bytes)
    }
    var sample: CMSampleBuffer?
    CMAudioSampleBufferCreateReadyWithPacketDescriptions(allocator: nil, dataBuffer: block!,
        formatDescription: description!, sampleCount: count,
        presentationTimeStamp: CMTime(value: CMTimeValue(start), timescale: CMTimeScale(sampleRate)),
        packetDescriptions: nil, sampleBufferOut: &sample)
    while !audio.isReadyForMoreMediaData { usleep(1000) }
    audio.append(sample!)
}

// One frame's worth of audio after each frame, so the writer can interleave.
let samplesPerFrame = Int(sampleRate) / Int(fps)

for index in 0..<frames {
    while !video.isReadyForMoreMediaData { usleep(1000) }
    var buffer: CVPixelBuffer?
    CVPixelBufferPoolCreatePixelBuffer(nil, adaptor.pixelBufferPool!, &buffer)
    let pixels = buffer!
    CVPixelBufferLockBaseAddress(pixels, [])
    let base = CVPixelBufferGetBaseAddress(pixels)!.assumingMemoryBound(to: UInt8.self)
    let stride = CVPixelBufferGetBytesPerRow(pixels)
    for y in 0..<height {
        for x in 0..<width {
            let p = base + y * stride + x * 4
            let bar = x == index % width
            p[0] = bar ? 255 : UInt8(y * 4)
            p[1] = bar ? 255 : UInt8(index * 4)
            p[2] = bar ? 255 : UInt8(x * 4)
            p[3] = 255
        }
    }
    CVPixelBufferUnlockBaseAddress(pixels, [])
    adaptor.append(pixels, withPresentationTime: CMTime(value: CMTimeValue(index), timescale: fps))
    appendAudio(from: index * samplesPerFrame, count: samplesPerFrame)
}
video.markAsFinished()
audio.markAsFinished()
print("finishing")

var finished = false
writer.finishWriting { finished = true }
// The completion may run on the main queue, so keep it turning.
while !finished { RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01)) }
if writer.status != .completed { fatalError("writer failed: \(String(describing: writer.error))") }
