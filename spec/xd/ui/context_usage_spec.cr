require "../../spec_helper"
require "../../../src/xd/ui/context_usage"

describe Xd::UI::ContextUsage do
  it "matches C token abbreviations" do
    usage = Xd::UI::ContextUsage

    usage.format_tokens(999_u64).should eq("999")
    usage.format_tokens(1_000_u64).should eq("1k")
    usage.format_tokens(1_200_u64).should eq("1.2k")
    usage.format_tokens(999_999_u64).should eq("1000.0k")
    usage.format_tokens(1_000_000_u64).should eq("1M")
    usage.format_tokens(1_200_000_u64).should eq("1.2M")
  end

  it "hides empty usage and clamps over-capacity percentages" do
    usage = Xd::UI::ContextUsage

    usage.meter(0_u64, 100_u64).should be_nil
    usage.meter(10_u64, 0_u64).should be_nil

    meter = usage.meter(120_u64, 100_u64).not_nil!
    meter.fraction.should eq(1.0)
    meter.label.should eq("120 / 100")
    meter.tooltip.should eq(
      "Context window: 120 of 100 tokens (100%)"
    )
  end

  it "uses the exact warning and error thresholds" do
    usage = Xd::UI::ContextUsage

    usage.meter(749_u64, 1_000_u64).not_nil!.severity
      .normal?.should be_true
    usage.meter(750_u64, 1_000_u64).not_nil!.severity
      .warning?.should be_true
    usage.meter(899_u64, 1_000_u64).not_nil!.severity
      .warning?.should be_true
    usage.meter(900_u64, 1_000_u64).not_nil!.severity
      .error?.should be_true
  end
end
