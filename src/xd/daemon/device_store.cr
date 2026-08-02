module Xd
  module Daemon
    class DeviceStoreError < Exception
    end

    record Device,
      id : String,
      name : String,
      created_at : Int64,
      last_seen : Int64

    abstract class DeviceStore
      DEFAULT_DEVICE_NAME = "Unknown device"
      MAX_DEVICE_NAME_BYTES = 80

      abstract def add_device(token_hash : String, name : String) : Nil
      abstract def device_name(token_hash : String) : String?
      abstract def list_devices : Array(Device)
      abstract def rename_device(token_hash : String, name : String) : Nil
      abstract def revoke_device(token_hash : String) : Nil

      def self.normalize_name(name : String) : String
        if name.each_byte.any? { |byte| byte < 0x20 || byte == 0x7f }
          raise DeviceStoreError.new(
            "A device name cannot contain control characters."
          )
        end
        normalized = name.strip
        if normalized.empty?
          raise DeviceStoreError.new("A device name cannot be empty.")
        end
        if normalized.bytesize > MAX_DEVICE_NAME_BYTES
          raise DeviceStoreError.new(
            "A device name cannot exceed #{MAX_DEVICE_NAME_BYTES} bytes."
          )
        end
        normalized
      end
    end

    # Test and bootstrap store. SQLiteStore replaces this in the executable;
    # both implement the same daemon-owned interface.
    class MemoryDeviceStore < DeviceStore
      getter devices = {} of String => String

      @timestamps = {} of String => {Int64, Int64}

      def add_device(token_hash : String, name : String) : Nil
        normalized = DeviceStore.normalize_name(
          name.empty? ? DeviceStore::DEFAULT_DEVICE_NAME : name
        )
        now = Time.utc.to_unix
        @devices[token_hash] = normalized
        @timestamps[token_hash] = {now, now}
      end

      def device_name(token_hash : String) : String?
        if name = @devices[token_hash]?
          created_at, _last_seen = @timestamps[token_hash]? || {0_i64, 0_i64}
          @timestamps[token_hash] = {created_at, Time.utc.to_unix}
          name
        end
      end

      def list_devices : Array(Device)
        @devices.map do |token_hash, name|
          created_at, last_seen = @timestamps[token_hash]? || {0_i64, 0_i64}
          Device.new(token_hash, name, created_at, last_seen)
        end
      end

      def rename_device(token_hash : String, name : String) : Nil
        unless @devices.has_key?(token_hash)
          raise DeviceStoreError.new("Unknown device.")
        end
        @devices[token_hash] = DeviceStore.normalize_name(name)
      end

      def revoke_device(token_hash : String) : Nil
        unless @devices.delete(token_hash)
          raise DeviceStoreError.new("Unknown device.")
        end
        @timestamps.delete(token_hash)
      end
    end
  end
end
