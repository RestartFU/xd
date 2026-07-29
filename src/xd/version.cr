module Xd
  VERSION = "0.1.0"

  BUILD_PROFILE = {{ env("XD_BUILD_PROFILE") || "default" }}
  BUILD_COMMIT  = {{ env("XD_BUILD_COMMIT") || "" }}

  {% if (env("XD_BUILD_PROFILE") || "default") == "nightly" %}
    APP_ID = "com.restartfu.Xd.Nightly"
    APP_NAME = "xd (Nightly)"
    DATA_NAME = "xd-nightly"
    SETTINGS_PATH = "/com/restartfu/XdNightly/"
  {% else %}
    APP_ID = "com.restartfu.Xd"
    APP_NAME = "xd"
    DATA_NAME = "xd"
    SETTINGS_PATH = "/com/restartfu/Hy/"
  {% end %}

  def self.version_string : String
    String.build do |value|
      value << VERSION
      value << "-nightly" if BUILD_PROFILE == "nightly"
      value << " (" << BUILD_COMMIT << ")" unless BUILD_COMMIT.empty?
    end
  end
end
