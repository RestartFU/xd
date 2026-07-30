require "../spec_helper"
require "../../src/xd/syntax_highlight"

describe Xd::SyntaxHighlight do
  it "prepares exact character ranges across stateful lines" do
    text = "int value; /* open\nstill comment */ return value;"
    spans = Xd::SyntaxHighlight.prepare("main.c", text)

    keyword = spans.find do |span|
      span.token.keyword? &&
        text[span.start...span.finish] == "return"
    end
    keyword.should_not be_nil

    comments = spans.select(&.token.comment?)
    text[comments.first.start...comments.last.finish]
      .should eq("/* open\nstill comment */")
  end

  it "skips plain text and obeys the line cap" do
    Xd::SyntaxHighlight.prepare("README.md", "# text").should be_empty

    spans = Xd::SyntaxHighlight.prepare(
      "main.c",
      "int first;\nint second;",
      line_limit: 1
    )
    spans.any? do |span|
      span.token.keyword? &&
        span.start == 0 &&
        span.finish == 3
    end.should be_true
    spans.none? { |span| span.start > 10 }.should be_true
  end
end
