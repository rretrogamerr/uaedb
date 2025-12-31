# UAEDB Portable (Windows)

## 설치

1. GitHub 릴리즈에서 portable zip을 다운로드합니다.
2. zip을 원하는 폴더에 압축 해제합니다.
3. `runtime/` 폴더를 `uaedb.exe` 옆에 그대로 둡니다.

## 사용 방법

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta`는 반드시 파일이어야 합니다. 디렉터리는 오류가 납니다.
번들에 엔트리가 여러 개면 `--entry`로 패치할 파일을 선택하세요
(`--list-entries`로 전체 경로 목록을 확인할 수 있습니다). `--entry`를
지정하지 않으면 모든 엔트리에 패치를 시도하고, 정확히 1개만 매칭돼야
합니다.

목록 확인 및 대상 선택:

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d --list-entries
uaedb original.unity3d patch.xdelta original_patched.unity3d --entry "data.unity3d/GI/level84/..."
```

## 참고

- `uaedb.exe`를 옮길 때는 `runtime/` 폴더도 함께 옮겨주세요.
- `xdelta3`를 찾지 못하면 `--xdelta`로 `xdelta3.exe`의 전체 경로를 지정하세요.
- `--keep-work` 옵션으로 보존된 작업 폴더 안의 `entry.bin`, `bundle.data`를 확인할 수 있습니다.
