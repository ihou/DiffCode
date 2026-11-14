// 用应用 Info.plist 里的 SUPublicEDKey 独立校验 Sparkle 的 EdDSA 签名：
// sign_update --verify 用的是钥匙串里的私钥推出来的公钥，发现不了“应用里的公钥和签名用的私钥不是一对”。
//
//   swift verify_ed_signature.swift <SUPublicEDKey> <sparkle:edSignature> <archive>
import CryptoKit
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 4,
      let keyData = Data(base64Encoded: arguments[1]),
      let signature = Data(base64Encoded: arguments[2]),
      let archive = FileManager.default.contents(atPath: arguments[3]) else {
    FileHandle.standardError.write(Data("usage: verify_ed_signature.swift <public-key-base64> <signature-base64> <file>\n".utf8))
    exit(2)
}
do {
    let key = try Curve25519.Signing.PublicKey(rawRepresentation: keyData)
    guard key.isValidSignature(signature, for: archive) else {
        FileHandle.standardError.write(Data("signature does not match SUPublicEDKey\n".utf8))
        exit(1)
    }
    print("signature matches SUPublicEDKey")
} catch {
    FileHandle.standardError.write(Data("invalid SUPublicEDKey: \(error)\n".utf8))
    exit(2)
}
