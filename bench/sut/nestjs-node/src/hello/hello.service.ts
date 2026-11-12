import { Injectable } from '@nestjs/common';

@Injectable()
export class HelloService {
  greeting(): string {
    return 'Hello World';
  }

  items(): { id: number; message: string }[] {
    return Array.from({ length: 1000 }, (_, id) => ({
      id,
      message: this.greeting(),
    }));
  }
}
