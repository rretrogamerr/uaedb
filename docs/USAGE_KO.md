# UAEDB Portable (Windows)

## 설치

1. GitHub 릴리즈에서 portable zip을 다운로드합니다.
2. zip을 원하는 폴더에 압축 해제합니다.
3. `runtime/`과 `scripts/` 폴더를 `uaedb.exe` 옆에 그대로 둡니다.

## 사용 방법

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta`는 반드시 파일이어야 합니다. 디렉터리는 오류가 납니다.
UAEDB는 언컴프레스 결과가 단일 파일일 때만 동작합니다.

## 참고

- `uaedb.exe`를 옮길 때는 `runtime/`, `scripts/` 폴더도 함께 옮겨주세요.
- `xdelta3`를 찾지 못하면 `--xdelta`로 `xdelta3.exe`의 전체 경로를 지정하세요.
- `--keep-work` 옵션으로 `workdir/unpack/files/` 경로에서 언컴프레스된 파일을 확인할 수 있습니다.
