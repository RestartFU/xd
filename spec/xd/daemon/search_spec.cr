require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/search"

private def with_search(
  & : Xd::Storage::Store, Xd::Daemon::Search ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-search-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))

  begin
    yield store, Xd::Daemon::Search.new(store)
  ensure
    store.close
    FileUtils.rm_r(directory)
  end
end

describe Xd::Daemon::Search do
  it "quotes FTS syntax and enables prefix matching" do
    Xd::Daemon::Search.fts_query(%( socket- "retry" )).should eq(
      %("socket-"* """retry"""*)
    )
    Xd::Daemon::Search.fts_query(" \t\n").should be_nil
  end

  it "returns display-ready chat hits" do
    with_search do |store, search|
      chat_id = store.create_chat("folder", "Networking", "claude")
      store.append_message(
        chat_id,
        "user",
        "the websocket reconnect loop\nneeds exponential backoff"
      )

      hits = search.call("websock reconn")
      hits.size.should eq(1)
      hits[0].chat_id.should eq(chat_id)
      hits[0].title.should eq("Networking")
      hits[0].role.should eq("user")
      hits[0].snippet.should contain("reconnect loop needs")
    end
  end

  it "caps and shortens results" do
    long = "x" * 130
    Xd::Daemon::Search.snippet(long).should eq("#{"x" * 120}…")
  end
end
