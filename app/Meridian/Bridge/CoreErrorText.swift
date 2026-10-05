import Foundation
import MeridianCore

extension Error {
    /// The core's user-facing sentence for an error, never the enum dump
    /// (`NotAvailable(reason: …)`).
    var userMessage: String {
        guard let e = self as? CoreError else { return localizedDescription }
        switch e {
        case let .NotAvailable(reason): return reason
        case let .InvalidInput(message): return message
        case let .NotFound(what): return "\(what) not found"
        case let .Provider(message): return message
        case let .Storage(message): return "Storage error: \(message)"
        case .Cancelled: return "Cancelled"
        case let .Internal(message): return "Internal error: \(message)"
        }
    }
}
