require "socket"

module Xd
  module Daemon
    module NetworkAddress
      extend self

      # Ask the kernel which IPv4 address its default LAN route would use.
      # UDP connect performs no network traffic until a packet is written.
      def local : String
        ["1.1.1.1", "8.8.8.8", "192.0.2.1"].each do |target|
          socket = UDPSocket.new
          begin
            socket.connect(target, 53)
            address = socket.local_address.address
            return address unless address.empty? ||
                                  address == "0.0.0.0" ||
                                  address.starts_with?("127.")
          rescue Socket::Error | IO::Error
          ensure
            socket.close
          end
        end
        System.hostname
      end
    end
  end
end
