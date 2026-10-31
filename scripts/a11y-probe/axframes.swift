// Frames of one process's accessibility tree: each element's AXFrame (screen points, y down from the top of the
// main display) and, for text elements, the bounds of the selected range and of the character after the caret, as
// VoiceOver and the zoom feature ask for them. Only the process given is read or acted on.
//   swiftc -O scripts/a11y-probe/axframes.swift -o /tmp/axframes
//   /tmp/axframes <pid> [--depth N] [--role AXRow] [--select LOC LEN] [--set-value TITLE TEXT] [--press TITLE] [--shot out.png]
// --select sets the first text area's AXSelectedTextRange first (as a screen reader does). --shot captures the
// process's largest window (screencapture -l) and draws every listed frame on it: elements in blue, the selection's
// bounds in red, the caret character's in green, so the frames can be checked against what is drawn.
import ApplicationServices
import Cocoa

var args = Array(CommandLine.arguments.dropFirst())
guard let first = args.first, let pid = pid_t(first) else {
  print("usage: axframes <pid> [--depth N] [--role AXRole] [--select LOC LEN] [--shot out.png]")
  exit(2)
}
args.removeFirst()
var maxDepth = 14
var filter: String?
var select: (Int, Int)?
var shot: String?
var setValue: (String, String)?
var press: String?
while !args.isEmpty {
  let flag = args.removeFirst()
  switch flag {
  case "--depth": maxDepth = Int(args.removeFirst()) ?? 14
  case "--role": filter = args.removeFirst()
  case "--select": select = (Int(args.removeFirst()) ?? 0, Int(args.removeFirst()) ?? 0)
  case "--shot": shot = args.removeFirst()
  case "--set-value": setValue = (args.removeFirst(), args.removeFirst())
  case "--press": press = args.removeFirst()
  default: print("unknown flag \(flag)"); exit(2)
  }
}

func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
  var v: CFTypeRef?
  return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? v : nil
}

func rect(_ v: AnyObject?) -> CGRect? {
  guard let v = v, CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
  var r = CGRect.zero
  return AXValueGetValue(v as! AXValue, .cgRect, &r) ? r : nil
}

func frame(_ e: AXUIElement) -> CGRect? {
  var p = CGPoint.zero
  var s = CGSize.zero
  guard let pv = attr(e, "AXPosition"), let sv = attr(e, "AXSize") else { return nil }
  AXValueGetValue(pv as! AXValue, .cgPoint, &p)
  AXValueGetValue(sv as! AXValue, .cgSize, &s)
  return CGRect(origin: p, size: s)
}

func bounds(_ e: AXUIElement, _ loc: Int, _ len: Int) -> CGRect? {
  var range = CFRange(location: loc, length: len)
  guard let value = AXValueCreate(.cfRange, &range) else { return nil }
  var out: CFTypeRef?
  guard AXUIElementCopyParameterizedAttributeValue(e, "AXBoundsForRange" as CFString, value, &out) == .success else { return nil }
  return rect(out)
}

func fmt(_ r: CGRect?) -> String {
  guard let r = r else { return "-" }
  return String(format: "(%.1f,%.1f %.1fx%.1f)", r.origin.x, r.origin.y, r.size.width, r.size.height)
}

func short(_ v: AnyObject?) -> String? {
  guard let v = v else { return nil }
  var s = "\(v)".replacingOccurrences(of: "\n", with: "⏎")
  if s.count > 48 { s = String(s.prefix(48)) + "…" }
  return s
}

func find(_ e: AXUIElement, _ role: String) -> AXUIElement? {
  if (attr(e, "AXRole") as? String) == role { return e }
  for k in attr(e, "AXChildren") as? [AXUIElement] ?? [] {
    if let r = find(k, role) { return r }
  }
  return nil
}

var drawn: [(CGRect, NSColor)] = []

func dump(_ e: AXUIElement, _ depth: Int) {
  let role = attr(e, "AXRole") as? String ?? "?"
  if filter == nil || role == filter {
    let f = frame(e)
    var parts = [role, fmt(f)]
    for k in ["AXTitle", "AXDescription", "AXValue"] {
      if let s = short(attr(e, k)), !s.isEmpty { parts.append("\(k.dropFirst(2))=\(s.debugDescription)") }
    }
    if let f = f, depth > 1, role != "AXWindow", role != "AXGroup" || filter != nil { drawn.append((f, .systemBlue)) }
    if let r = attr(e, "AXSelectedTextRange") {
      var rg = CFRange()
      AXValueGetValue(r as! AXValue, .cfRange, &rg)
      let sel = bounds(e, rg.location, rg.length)
      let ch = bounds(e, rg.location, 1)
      parts.append("sel=\(rg.location)+\(rg.length) selBounds=\(fmt(sel)) charBounds=\(fmt(ch))")
      if let ch = ch { drawn.append((ch, .systemGreen)) }
      if let sel = sel { drawn.append((sel, .systemRed)) }
    }
    print(String(repeating: "  ", count: filter == nil ? depth : 0) + parts.joined(separator: " "))
  }
  if depth >= maxDepth { return }
  for k in attr(e, "AXChildren") as? [AXUIElement] ?? [] { dump(k, depth + 1) }
}

/// The process's on-screen window that holds `focus` (the smallest one that contains it), else its largest window: its
/// id and its bounds in screen points.
func shotWindow(_ focus: CGRect?) -> (CGWindowID, CGRect)? {
  let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
  var best: (CGWindowID, CGRect)?
  for w in info where (w[kCGWindowOwnerPID as String] as? pid_t) == pid {
    guard let id = w[kCGWindowNumber as String] as? CGWindowID,
      let b = w[kCGWindowBounds as String] as? [String: CGFloat],
      let x = b["X"], let y = b["Y"], let width = b["Width"], let height = b["Height"]
    else { continue }
    let r = CGRect(x: x, y: y, width: width, height: height)
    let area = r.width * r.height
    if let f = focus {
      if r.contains(f) && (best == nil || area < best!.1.width * best!.1.height) { best = (id, r) }
    } else if best == nil || area > best!.1.width * best!.1.height {
      best = (id, r)
    }
  }
  return best
}

print("AXIsProcessTrusted:", AXIsProcessTrusted())
let app = AXUIElementCreateApplication(pid)
/// The first element titled `title` (AXTitle or AXDescription).
func titled(_ e: AXUIElement, _ title: String) -> AXUIElement? {
  if (attr(e, "AXTitle") as? String) == title || (attr(e, "AXDescription") as? String) == title { return e }
  for k in attr(e, "AXChildren") as? [AXUIElement] ?? [] {
    if let r = titled(k, title) { return r }
  }
  return nil
}
/// The first element titled `title` whose value can be set.
func settable(_ e: AXUIElement, _ title: String) -> AXUIElement? {
  var ok = DarwinBoolean(false)
  if (attr(e, "AXTitle") as? String) == title && (attr(e, "AXRole") as? String) != "AXGroup" && AXUIElementIsAttributeSettable(e, "AXValue" as CFString, &ok) == .success && ok.boolValue { return e }
  for k in attr(e, "AXChildren") as? [AXUIElement] ?? [] {
    if let r = settable(k, title) { return r }
  }
  return nil
}
// Only the process's windows: the menu bar has fields of its own (Help's search).
let windows = attr(app, "AXWindows") as? [AXUIElement] ?? []
if let (title, text) = setValue, let e = windows.lazy.compactMap({ settable($0, title) }).first {
  print("set AXValue of \(title) (\(attr(e, "AXRole") as? String ?? "?") \(fmt(frame(e)))) -> \(AXUIElementSetAttributeValue(e, "AXValue" as CFString, text as CFString).rawValue)")
  Thread.sleep(forTimeInterval: 1.5)
}
if let title = press, let e = windows.lazy.compactMap({ titled($0, title) }).first {
  print("AXPress \(title) -> \(AXUIElementPerformAction(e, kAXPressAction as CFString).rawValue)")
  Thread.sleep(forTimeInterval: 1.0)
}
let focused = attr(app, "AXFocusedUIElement").map { $0 as! AXUIElement }
let focusedArea = focused.flatMap { (attr($0, "AXRole") as? String) == "AXTextArea" ? $0 : nil }
if let (loc, len) = select, let area = focusedArea ?? find(app, "AXTextArea") {
  var range = CFRange(location: loc, length: len)
  let err = AXUIElementSetAttributeValue(area, "AXSelectedTextRange" as CFString, AXValueCreate(.cfRange, &range)!)
  print("set AXSelectedTextRange \(loc)+\(len) -> \(err.rawValue)")
  Thread.sleep(forTimeInterval: 1.0)
}
for w in attr(app, "AXWindows") as? [AXUIElement] ?? [] {
  print("WINDOW \(short(attr(w, "AXTitle")) ?? "") \(fmt(frame(w)))")
}
if let f = attr(app, "AXFocusedUIElement") {
  let e = f as! AXUIElement
  print("FOCUSED \(attr(e, "AXRole") as? String ?? "?") \(short(attr(e, "AXTitle")) ?? "") \(fmt(frame(e)))")
}
for w in attr(app, "AXWindows") as? [AXUIElement] ?? [] { dump(w, 1) }

if let out = shot, let (id, win) = shotWindow(focused.flatMap(frame)) ?? shotWindow(nil) {
  let raw = out + ".raw.png"
  let p = Process()
  p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
  p.arguments = ["-x", "-o", "-l\(id)", raw]
  try? p.run()
  p.waitUntilExit()
  guard let image = NSImage(contentsOfFile: raw), let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    print("no screenshot")
    exit(1)
  }
  let scale = CGFloat(cg.width) / win.width
  let ctx = CGContext(data: nil, width: cg.width, height: cg.height, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(cg, in: CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
  ctx.setLineWidth(max(1, scale))
  for (r, color) in drawn {
    // Screen points (y down) → image pixels (y up).
    let x = (r.minX - win.minX) * scale
    let y = CGFloat(cg.height) - (r.maxY - win.minY) * scale
    ctx.setStrokeColor(color.withAlphaComponent(color == .systemBlue ? 0.55 : 1).cgColor)
    ctx.stroke(CGRect(x: x, y: y, width: max(r.width * scale, 1), height: r.height * scale))
  }
  let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
  try? rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
  try? FileManager.default.removeItem(atPath: raw)
  print("SHOT \(out) window=\(fmt(win)) scale=\(scale) frames=\(drawn.count)")
}
