// A minimal App Store Connect API client for what the signing setup needs: bundle IDs, certificates and
// provisioning profiles. Prints the JSON response; exits 1 on an HTTP error (the body goes to stderr).
//
//   ASC_KEY_ID=… ASC_ISSUER_ID=… ASC_KEY_PATH=AuthKey_….p8 swift asc.swift GET '/v1/bundleIds?filter[identifier]=com.nanshanbei.DiffCode'
//   … swift asc.swift POST /v1/profiles body.json
//
// The private key is read from ASC_KEY_PATH and used only to sign a 20-minute token; it is never printed.
import CryptoKit
import Foundation

func fail(_ message: String, code: Int32 = 2) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

func base64URL(_ data: Data) -> String {
    data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
}

let env = ProcessInfo.processInfo.environment
guard let keyID = env["ASC_KEY_ID"], let issuer = env["ASC_ISSUER_ID"], let keyPath = env["ASC_KEY_PATH"] else {
    fail("set ASC_KEY_ID, ASC_ISSUER_ID and ASC_KEY_PATH")
}
let arguments = CommandLine.arguments
guard arguments.count >= 3 else { fail("usage: asc.swift GET|POST|PATCH|DELETE /v1/path [body.json]") }
let method = arguments[1].uppercased()
let path = arguments[2]

guard let pem = try? String(contentsOfFile: keyPath, encoding: .utf8),
      let key = try? P256.Signing.PrivateKey(pemRepresentation: pem) else {
    fail("cannot read an ES256 private key from \(keyPath)")
}
let now = Int(Date().timeIntervalSince1970)
let header = try! JSONSerialization.data(withJSONObject: ["alg": "ES256", "kid": keyID, "typ": "JWT"])
let claims = try! JSONSerialization.data(withJSONObject: ["iss": issuer, "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1"])
let signingInput = base64URL(header) + "." + base64URL(claims)
guard let signature = try? key.signature(for: Data(signingInput.utf8)) else { fail("cannot sign the token") }
let token = signingInput + "." + base64URL(signature.rawRepresentation)

guard let url = URL(string: "https://api.appstoreconnect.apple.com" + path) else { fail("bad path \(path)") }
var request = URLRequest(url: url)
request.httpMethod = method
request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
if arguments.count >= 4 {
    guard let body = FileManager.default.contents(atPath: arguments[3]) else { fail("cannot read \(arguments[3])") }
    request.httpBody = body
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
}

let done = DispatchSemaphore(value: 0)
var status = 0
var payload = Data()
URLSession.shared.dataTask(with: request) { data, response, error in
    if let error { fail("request failed: \(error.localizedDescription)") }
    status = (response as? HTTPURLResponse)?.statusCode ?? 0
    payload = data ?? Data()
    done.signal()
}.resume()
done.wait()

if (200..<300).contains(status) {
    FileHandle.standardOutput.write(payload)
    exit(0)
}
FileHandle.standardError.write(Data("HTTP \(status): ".utf8) + payload + Data("\n".utf8))
exit(1)
