// #6 の骨格 — サイドバー（ダッシュボード ＋ `Person` ＋ 設定）とメイン。
//
// ダッシュボードと設定は項目だけ置く。中身は別のスライス（#30）。

import EscrowBridge
import SwiftUI

struct ContentView: View {
  @State private var phase: Phase = .opening

  var body: some View {
    Group {
      switch phase {
      case .opening:
        Text("開いています")
      case .unavailable(let why):
        Text("開けません — \(why)")
      case .ready(let escrow, let persons):
        Skeleton(escrow: escrow, persons: persons)
      }
    }
    .task {
      do {
        let escrow = try await openEscrow()
        phase = .ready(escrow, try await persons(escrow: escrow))
      } catch {
        phase = .unavailable(why(error))
      }
    }
  }
}

/// イベントストアを開いたあと。
private struct Skeleton: View {
  let escrow: Escrow
  let persons: [Person]
  @State private var selection: Selection? = .dashboard

  var body: some View {
    NavigationSplitView {
      List(selection: $selection) {
        Label("ダッシュボード", systemImage: "square.grid.2x2").tag(Selection.dashboard)
        Section("持ち主") {
          ForEach(persons, id: \.id) { person in
            Label(person.name, systemImage: "person").tag(Selection.person(person.id))
          }
        }
        Section {
          Label("設定", systemImage: "gearshape").tag(Selection.settings)
        }
      }
      .navigationSplitViewColumnWidth(min: 180, ideal: 200)
    } detail: {
      switch selection {
      case .dashboard, .none:
        Text("ダッシュボードは別のスライスで入る")
      case .settings:
        Text("設定は別のスライスで入る")
      case .person(let id):
        if let person = persons.first(where: { $0.id == id }) {
          ItemsOf(escrow: escrow, person: person)
        } else {
          // 持ち主を読んだあとに消えた。次に開けば居なくなっている。
          Text("この持ち主は居ない")
        }
      }
    }
  }
}

/// 選んだ持ち主の項目一覧 — 日付・見出し・状態・種別（#6）。
private struct ItemsOf: View {
  let escrow: Escrow
  let person: Person
  @State private var listing: Listing = .loading

  var body: some View {
    Group {
      switch listing {
      case .loading:
        Text("読んでいます")
      case .failed(let why):
        Text("項目を読めません — \(why)")
      case .loaded(let listed) where listed.isEmpty:
        Text("項目はまだ無い")
      case .loaded(let listed):
        Table(rows(listed)) {
          TableColumn("日付", value: \.item.publishedOn).width(90)
          TableColumn("項目") { row in
            // 何文字で切るかは列幅が決める（#82）。`title` も `body` の1行目も同じ扱い。
            Text(shown(row.item.headline)).lineLimit(1).truncationMode(.tail)
          }
          TableColumn("状態", value: \.item.state).width(100)
          TableColumn("種別", value: \.item.contentType).width(120)
        }
      }
    }
    .navigationTitle(person.name)
    // 選び直すと前の読み出しは取り消される。橋の向こうの読み出しは止まらないので、
    // 届いたぶんを捨てるのはここ。
    .task(id: person.id) {
      listing = .loading
      do {
        let listed = try await itemsOf(escrow: escrow, person: person.id)
        if !Task.isCancelled {
          listing = .loaded(listed)
        }
      } catch {
        if !Task.isCancelled {
          listing = .failed(why(error))
        }
      }
    }
  }
}

/// 一覧の1行。`Table` は行の識別子を求めるが、`Listed` は id を持たないので並び順で代用する。
/// 行を選んだり並べ替えたりする段になったら、`escrow-app` の `Listed` に `ItemId` を載せる。
private struct Row: Identifiable {
  let id: Int
  let item: Listed
}

private func rows(_ listed: [Listed]) -> [Row] {
  listed.enumerated().map { Row(id: $0.offset, item: $0.element) }
}

/// 見出し。`Media` は `title`、`Post` は `body` の1行目（#6）。
private func shown(_ headline: Headline) -> String {
  switch headline {
  case .title(let title): title
  case .opening(let opening): opening
  }
}
