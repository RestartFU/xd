require "../../spec_helper"

describe "desktop dialogs" do
  it "does not use laggy Adwaita alert dialogs" do
    offenders = Dir.glob("src/**/*.cr").select do |path|
      File.read(path).includes?("Adw::AlertDialog")
    end

    offenders.should be_empty
  end
end
