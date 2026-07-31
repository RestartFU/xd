require "../../spec_helper"
require "../../../src/xd/agent/environment"
require "../../../src/xd/agent/executable"

describe Xd::Agent::Environment do
  it "restores host values and strips bundle-only state" do
    source = {
      "GTK_PATH"                     => "/bundle/gtk",
      "XD_HOST_GTK_PATH"             => "/host/gtk",
      "LOCPATH"                      => "/bundle/locale",
      "XD_HOST_LOCPATH"              => "",
      "XD_HOST_LOCALE_ARCHIVE"       => "/host/locale-archive",
      "GSETTINGS_SCHEMA_DIR"         => "/bundle/schemas",
      "XD_HOST_GSETTINGS_SCHEMA_DIR" => "/host/schemas",
      "PATH"                         => "/bundle/bin:/host/bin",
      "XD_HOST_PATH"                 => "/host/bin",
      "SSL_CERT_FILE"                => "/bundle/ca.pem",
      "XD_HOST_SSL_CERT_FILE"        => "/host/corporate-ca.pem",
      "OPENSSL_MODULES"              => "/bundle/modules",
      "XD_HOST_OPENSSL_MODULES"      => "",
      "XD_AGENT_SECRETS_FILE"        => "/private/secrets.json",
      "UNCHANGED"                    => "yes",
    }

    environment = Xd::Agent::Environment.host(source)
    environment["GTK_PATH"].should eq("/host/gtk")
    environment.has_key?("LOCPATH").should be_false
    environment["LOCALE_ARCHIVE"].should eq("/host/locale-archive")
    environment["GSETTINGS_SCHEMA_DIR"].should eq("/host/schemas")
    environment["PATH"].should eq("/host/bin")
    environment["SSL_CERT_FILE"].should eq("/host/corporate-ca.pem")
    environment.has_key?("OPENSSL_MODULES").should be_false
    environment.has_key?("XD_HOST_GTK_PATH").should be_false
    environment.has_key?("XD_HOST_SSL_CERT_FILE").should be_false
    environment.has_key?("XD_AGENT_SECRETS_FILE").should be_false
    environment["UNCHANGED"].should eq("yes")
  end

  it "lets Codex inherit only explicit secret-looking names" do
    names = Xd::Agent::Environment.allowed_names({
      "PATH"         => "/bin",
      "NORMAL"       => "value",
      "SYSTEM_TOKEN" => "hidden",
      "CUSTOM_TOKEN" => "allowed",
      "SIGNING_KEY"  => "hidden",
      "API_SECRET"   => "hidden",
    }, ["CUSTOM_TOKEN"])

    names.should eq(["CUSTOM_TOKEN", "NORMAL", "PATH"])
    Xd::Agent::Environment.allowed_names(
      {"PATH" => "/bin"},
      [] of String
    ).should be_nil
  end

  it "keys persistent servers by executable and full environment" do
    first = Xd::Agent::Environment.pool_key(
      "/app/codex",
      {"B" => "2", "A" => "1"}
    )
    reordered = Xd::Agent::Environment.pool_key(
      "/app/codex",
      {"A" => "1", "B" => "2"}
    )
    changed = Xd::Agent::Environment.pool_key(
      "/app/codex",
      {"A" => "1", "B" => "3"}
    )

    first.should eq(reordered)
    first.should_not eq(changed)
  end
end
