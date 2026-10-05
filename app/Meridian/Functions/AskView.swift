import MeridianCore
import SwiftUI

/// ASK — the AI analyst. The Rust side (tool loop, audit, number verifier)
/// is wired through `AskService`; this view streams the answer and shows
/// numbered sources and unverified numbers.
struct AskView: View {
    @Bindable var panel: PanelModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("ASK — AI Analyst").font(Theme.swiftFont(weight: .bold)).foregroundStyle(Theme.white.swiftUI)
            Text("ASK is connected in Phase 8 of the build. It needs an Anthropic API key (Settings → API Keys).")
                .font(Theme.swiftFont())
                .foregroundStyle(Theme.amber.swiftUI)
            Spacer()
        }
        .padding(6)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
