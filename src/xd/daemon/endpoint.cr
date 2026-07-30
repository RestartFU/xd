require "json"

module Xd
  module Daemon
    # A source of daemon calls and events.
    #
    # Local Client and reconnecting remote Connection expose this same surface,
    # so UI code never grows transport-specific request logic.
    abstract class Endpoint
      abstract def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)

      abstract def subscribe(
        &subscriber : Hash(String, JSON::Any) ->
      ) : Int64

      abstract def unsubscribe(id : Int64) : Nil
      abstract def close : Nil
    end
  end
end
