require "json"
require "./operation"

module Xd
  module Protocol
    REQUEST_ID = "_xd_request"

    class Error < Exception
    end

    class Request
      getter operation : Operation
      getter body : Hash(String, JSON::Any)

      def initialize(@operation, @body)
      end

      def self.parse(line : String) : self
        root = JSON.parse(line)
        body = root.as_h?
        raise Error.new("Not a JSON object") unless body

        name = body["op"]?.try(&.as_s?)
        operation = name.try { |value| Operation.from_wire?(value) }
        raise Error.new("Unknown op") unless operation

        new(operation, body)
      rescue JSON::ParseException
        raise Error.new("Not a JSON object")
      end

      def string?(name : String) : String?
        @body[name]?.try(&.as_s?)
      end

      def int?(name : String) : Int64?
        @body[name]?.try(&.as_i64?)
      end

      def member?(name : String) : Bool
        @body.has_key?(name)
      end

      def string(name : String, message : String) : String
        string?(name) || raise Error.new(message)
      end
    end

    class Response
      getter body : Hash(String, JSON::Any)

      private def initialize(@body)
      end

      def self.ok(fields = {} of String => JSON::Any) : self
        body = {"ok" => JSON::Any.new(true)}
        fields.each { |name, value| body[name] = value }
        new(body)
      end

      def self.error(message : String) : self
        new({
          "ok"    => JSON::Any.new(false),
          "error" => JSON::Any.new(message),
        })
      end

      def success? : Bool
        @body["ok"].as_bool
      end

      def [](name : String) : JSON::Any
        @body[name]
      end

      def to_json(io : IO) : Nil
        @body.to_json(io)
      end

      def to_json : String
        String.build { |io| to_json(io) }
      end
    end

    class Event
      getter body : Hash(String, JSON::Any)
      getter audience : UInt64?

      def initialize(
        name : String,
        id : Int64,
        fields,
        @audience : UInt64? = nil,
      )
        @body = {
          "event" => JSON::Any.new(name),
          "id"    => JSON::Any.new(id),
        }
        fields.each { |key, value| @body[key] = value }
      end

      def [](name : String) : JSON::Any
        @body[name]
      end

      def to_json(io : IO) : Nil
        @body.to_json(io)
      end

      def to_json : String
        String.build { |io| to_json(io) }
      end
    end

    record Outcome,
      response : Response,
      events : Array(Event),
      after_write : Proc(Nil)? = nil
  end
end
