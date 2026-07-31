require "json"

module Xd
  module UI
    record PanelCallResult,
      body : Hash(String, JSON::Any)?,
      error : String?

    alias PanelCall = Proc(
      Hash(String, JSON::Any),
      PanelCallResult,
    )
  end
end
