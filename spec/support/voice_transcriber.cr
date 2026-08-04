require "../../src/xd/voice/transcriber"

module XdSpec
  class VoiceTranscriber < Xd::Voice::Transcriber
    def initialize(@text : String)
      super()
    end

    def warm(_model_path : String) : Nil
    end

    def transcribe(
      _wav : Bytes,
      _model_path : String,
      &finished : Xd::Voice::Transcription -> Nil
    ) : Nil
      spawn do
        finished.call(Xd::Voice::Transcription.new(@text, nil, false))
      end
    end

    def close : Nil
    end
  end
end
