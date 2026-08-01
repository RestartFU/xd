require "../../spec_helper"
require "../../../src/xd/ui/diff_file_sections"

describe Xd::UI::DiffFileSections do
  it "caps each expanded file before GTK widget creation" do
    plan = Xd::UI::DiffFileSections.render_plan(1, 10_001)

    plan.finish.should eq(4_001)
    plan.omitted.should eq(6_000)
  end

  it "keeps small file sections complete" do
    plan = Xd::UI::DiffFileSections.render_plan(3, 103)

    plan.finish.should eq(103)
    plan.omitted.should eq(0)
  end

  it "prepares syntax chunks before GTK rendering" do
    rows = (1..161).map { |line| "+puts #{line}" }.join('\n')
    patch = "diff --git a/main.cr b/main.cr\n" \
            "@@ -0,0 +1,161 @@\n#{rows}\n"

    prepared = Xd::UI::DiffFileSections.prepare(patch)
    body = prepared.bodies[prepared.sections.first.start]

    body.chunks.size.should eq(3)
    body.chunks.map(&.row_kinds.size).should eq([80, 80, 2])
    body.chunks.first.markup.should contain("puts")
    body.omitted.should eq(0)
  end

  it "shares one render budget across every changed file" do
    patch = (1..3).map do |file|
      rows = (1..2_000).map { |line| "+value_#{file}_#{line}" }.join('\n')
      "diff --git a/file#{file}.txt b/file#{file}.txt\n" \
      "@@ -0,0 +1,2000 @@\n#{rows}"
    end.join('\n')

    prepared = Xd::UI::DiffFileSections.prepare(patch)
    rendered = prepared.bodies.values.sum do |body|
      body.chunks.sum(&.row_kinds.size)
    end
    omitted = prepared.bodies.values.sum(&.omitted)

    rendered.should eq(Xd::UI::DiffFileSections::MAX_RENDERED_ROWS)
    omitted.should eq(2_003)
  end
end
