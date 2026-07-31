require "../../spec_helper"
require "digest/sha256"
require "file_utils"
require "http/server"
require "random/secure"
require "../../../src/xd/daemon/voice_jobs"

private class ThreadProbeVoiceModel < Xd::Voice::Model
  @worker : Thread?

  def initialize
    super(override_path: "/xd-test-model-does-not-exist")
    @worker = nil
    @mutex = Mutex.new
  end

  def find : String?
    nil
  end

  def ensure_available(&on_progress : Int32 -> Nil) : String
    @mutex.synchronize { @worker = Thread.current }
    on_progress.call(42)
    sleep 150.milliseconds
    on_progress.call(100)
    "/xd-test-model"
  end

  def worker : Thread?
    @mutex.synchronize { @worker }
  end
end

private class InstalledVoiceModel < Xd::Voice::Model
  def initialize
    super(override_path: "/xd-test-model")
  end

  def find : String?
    "/xd-test-model"
  end
end

private class StalledVoiceTranscriber < Xd::Voice::Transcriber
  def initialize
    super(resolver: -> { "/xd-test-whisper" })
    @was_cancelled = Atomic(Bool).new(false)
  end

  def transcribe(
    _wav : Bytes,
    _model_path : String,
    &finished : Xd::Voice::Transcription -> Nil
  ) : Nil
  end

  def cancel : Nil
    @was_cancelled.set(true)
  end

  def cancelled? : Bool
    @was_cancelled.get
  end
end

describe Xd::Daemon::VoiceJobs do
  it "cancels a stalled transcription and releases its request token" do
    events = Channel(Hash(String, JSON::Any)).new(4)
    transcribers = [] of StalledVoiceTranscriber
    jobs = Xd::Daemon::VoiceJobs.new(
      ->(_name : String, fields : Hash(String, JSON::Any), _owner : UInt64) { events.send(fields) },
      model_factory: -> {
        InstalledVoiceModel.new.as(Xd::Voice::Model)
      },
      transcriber_factory: -> {
        transcriber = StalledVoiceTranscriber.new
        transcribers << transcriber
        transcriber.as(Xd::Voice::Transcriber)
      },
      transcription_timeout: 25.milliseconds
    )
    audio = Base64.strict_encode(Bytes[1, 2, 3, 4])

    2.times do
      jobs.transcribe(11_u64, "repeat-token", audio)
      select
      when event = events.receive
        event["state"].as_s.should eq("error")
        event["error"].as_s.should contain("timed out")
      when timeout(1.second)
        fail("stalled transcription did not time out")
      end
    end

    transcribers.size.should eq(2)
    transcribers.all?(&.cancelled?).should be_true
  ensure
    jobs.try(&.close)
  end

  it "downloads on an OS thread and publishes from the daemon scheduler" do
    model = ThreadProbeVoiceModel.new
    events = [] of Hash(String, JSON::Any)
    event_threads = [] of Thread
    mutex = Mutex.new
    ready = Channel(Nil).new(1)
    publisher = ->(_name : String, fields : Hash(String, JSON::Any), _owner : UInt64) {
      mutex.synchronize do
        events << fields
        event_threads << Thread.current
      end
      ready.send(nil) if fields["state"].as_s == "ready"
    }
    jobs = Xd::Daemon::VoiceJobs.new(
      publisher,
      model_factory: -> { model.as(Xd::Voice::Model) }
    )
    caller = Thread.current

    jobs.download(7_u64, "thread-probe")
    select
    when ready.receive
    when timeout(2.seconds)
      fail("speech model download did not finish")
    end

    worker = model.worker
    worker.should_not be_nil
    worker.not_nil!.same?(caller).should be_false

    states, publishers = mutex.synchronize do
      {
        events.map { |event| event["state"].as_s },
        event_threads.dup,
      }
    end
    states.should contain("downloading")
    states.last.should eq("ready")
    publishers.any?(&.same?(worker.not_nil!)).should be_false
  ensure
    jobs.try(&.close)
  end

  it "downloads through HTTP inside the isolated execution context" do
    directory = File.join(
      Dir.tempdir,
      "xd-voice-jobs-#{Random::Secure.hex(12)}"
    )
    path = File.join(directory, "model.bin")
    payload = Bytes.new(1024 * 1024, 0x6d_u8)
    server = HTTP::Server.new do |context|
      context.response.content_length = payload.size
      context.response.write(payload)
    end
    address = server.bind_tcp("127.0.0.1", 0)
    spawn server.listen
    model = Xd::Voice::Model.new(
      path: path,
      url: "http://127.0.0.1:#{address.port}/model",
      expected_size: payload.size.to_u64,
      expected_sha256: Digest::SHA256.hexdigest(payload)
    )
    events = Channel(Hash(String, JSON::Any)).new(128)
    jobs = Xd::Daemon::VoiceJobs.new(
      ->(_name : String, fields : Hash(String, JSON::Any), _owner : UInt64) { events.send(fields) },
      model_factory: -> { model }
    )

    jobs.download(9_u64, "http-probe")
    state = ""
    until state == "ready"
      select
      when event = events.receive
        state = event["state"].as_s
      when timeout(3.seconds)
        fail("isolated HTTP download did not finish")
      end
    end

    File.read(path).to_slice.should eq(payload)
  ensure
    jobs.try(&.close)
    server.try(&.close)
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end
end
