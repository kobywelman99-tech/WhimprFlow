import UIKit

/// Opens the WhimprFlow app from the keyboard.
///
/// `UIApplication.open` is off-limits in extensions, but the UIApplication
/// object is still reachable and still answers
/// `openURL:options:completionHandler:`. Other dictation keyboards rely on the
/// same thing. If a future iOS closes it, `open` returns false and the keyboard
/// asks the user to open WhimprFlow themselves.
@MainActor
enum AppOpener {
    private typealias OpenURL = @convention(c) (AnyObject, Selector, NSURL, NSDictionary, AnyObject?) -> Void

    static func open(_ url: URL, from responder: UIResponder) -> Bool {
        let selector = NSSelectorFromString("openURL:options:completionHandler:")
        guard let application = findApplication(from: responder),
              application.responds(to: selector) else { return false }
        let open = unsafeBitCast(application.method(for: selector), to: OpenURL.self)
        open(application, selector, url as NSURL, NSDictionary(), nil)
        return true
    }

    private static func findApplication(from responder: UIResponder) -> UIApplication? {
        var current: UIResponder? = responder
        while let next = current {
            if let application = next as? UIApplication { return application }
            current = next.next
        }
        return (UIApplication.self as AnyObject).value(forKey: "sharedApplication") as? UIApplication
    }
}
