import { describe, expect, it } from "vitest";
import { parseMediaName, splitMiddle } from "../src/lib/mediaName";

const EPISODE = "The Marvelous Mrs. Maisel (2017) - S01E02 - Ya Shivu v Bolshom Dome Na Kholme [AMZN WEBDL-2160p][DV HDR10Plus][EAC3 Atmos 5.1][h265]-NTb";

describe("parseMediaName", () => {
  it("zerlegt einen Sonarr-Episodennamen", () => {
    expect(parseMediaName(EPISODE)).toEqual({
      matched: true,
      original: EPISODE,
      series: "The Marvelous Mrs. Maisel",
      year: "2017",
      code: "S01E02",
      title: "Ya Shivu v Bolshom Dome Na Kholme",
      ids: [],
      tags: ["AMZN WEBDL-2160p", "DV HDR10Plus", "EAC3 Atmos 5.1", "h265"],
      group: "NTb",
    });
  });

  it("erkennt {tvdb-…} und {imdb-…} als Kennungen", () => {
    const parsed = parseMediaName("Severance (2022) {tvdb-371980} - S02E09 - Cold Harbor [ATVP WEBDL-2160p]-FLUX");
    expect(parsed).toMatchObject({ series: "Severance", year: "2022", ids: ["tvdb-371980"], code: "S02E09", title: "Cold Harbor", group: "FLUX" });
    expect(parseMediaName("Dune (2021) {imdb-tt1160419} [Bluray-2160p]-GRP")).toMatchObject({ ids: ["imdb-tt1160419"] });
  });

  it("zerlegt einen Radarr-Filmnamen ohne Folge", () => {
    const parsed = parseMediaName("Dune (2021) {imdb-tt1160419} [Bluray-2160p][h265]-GRP");
    expect(parsed).toMatchObject({ matched: true, series: "Dune", year: "2021", tags: ["Bluray-2160p", "h265"], group: "GRP" });
    expect(parsed).toHaveProperty("code", undefined);
    expect(parsed).toHaveProperty("title", undefined);
  });

  it("nimmt Mehrfachfolgen und Folge ohne Titel an", () => {
    expect(parseMediaName("Show (2020) - S01E01-E02 - Pilot")).toMatchObject({ code: "S01E01-E02", title: "Pilot" });
    expect(parseMediaName("Show (2020) - S03E10")).toMatchObject({ code: "S03E10", title: undefined });
  });

  it("lässt Namen ohne Muster unverändert", () => {
    for (const name of ["holiday", "IMG_0042", "", "Some Movie", "[tag] only"]) {
      expect(parseMediaName(name)).toEqual({ matched: false, original: name });
    }
  });

  it("bleibt bei Randfällen defensiv", () => {
    expect(parseMediaName("Show - s01e02 - kleiner Titel")).toMatchObject({ matched: true, series: "Show", code: "S01E02", title: "kleiner Titel" });
    expect(parseMediaName("Show (2020) - S01E01 - Title [unbalanced")).toMatchObject({ matched: true, title: "Title [unbalanced", tags: [] });
  });
});

describe("splitMiddle", () => {
  it("behält Anfang und Ende", () => {
    expect(splitMiddle("S01E02 Ya Shivu v Bolshom Dome -NTb", 4)).toEqual({ head: "S01E02 Ya Shivu v Bolshom Dome ", tail: "-NTb" });
  });

  it("gibt kurze Namen ganz zurück", () => {
    expect(splitMiddle("Pilot", 8)).toEqual({ head: "Pilot", tail: "" });
  });
});
